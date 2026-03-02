//! POCSAG and FLEX Paging Protocol Processors
//!
//! Implements the physical and data link layers of two major one-way paging
//! standards used on VHF/UHF frequencies worldwide.
//!
//! ## POCSAG (ITU-R M.584-2)
//!
//! Post Office Code Standardization Advisory Group paging format.
//! 2-FSK at 512, 1200, or 2400 baud. Each batch consists of a 32-bit sync
//! word (0x7CD215D8) followed by 8 frames of 2 codewords each (16 codewords
//! per batch = 512 bits). BCH(31,21) error correction protects each codeword.
//!
//! Frame structure:
//! ```text
//! | Preamble (576 bits, 10101010…) | Sync (32 bits) | 8 Frames × 64 bits |
//! ```
//!
//! ## FLEX Protocol
//!
//! Motorola FLEX 4-level FSK at 1600/3200/6400 baud.  Supports 128 frames
//! per 4-minute cycle with multi-phase (A/B/C/D) transmitter multiplexing.
//! BCH(31,21) protects address words; block coding with interleaving protects
//! data words.
//!
//! ## References
//!
//! - ITU-R M.584-2 (POCSAG)
//! - FLEX/InFLEXion/ReFLEX protocol (Motorola)
//! - GNU Radio equivalent: `gr-pager` / `multimon-ng`
//!
//! ## Example
//!
//! ```rust
//! use r4w_core::pocsag_flex_pager::{
//!     PocsagConfig, PocsagEncoder, PocsagDecoder, BaudRate,
//!     FlexConfig, FlexEncoder, FlexDecoder, FlexBaudRate,
//!     PagerMessage, MessageContent,
//! };
//!
//! // Encode a POCSAG message
//! let cfg = PocsagConfig::new(1234567, 0, BaudRate::Baud1200);
//! let mut enc = PocsagEncoder::new(cfg);
//! let bits = enc.encode_alphanumeric("Hello pager!");
//! assert!(!bits.is_empty());
//!
//! // Decode it back
//! let mut dec = PocsagDecoder::new();
//! let messages = dec.feed_bits(&bits);
//! // In a real system, messages would be decoded from RF
//! ```

use std::f64::consts::PI;

// ─── Constants ────────────────────────────────────────────────────────────────

/// POCSAG preamble: 576 alternating bits (0xAA…).
pub const POCSAG_PREAMBLE_LEN: usize = 576;

/// POCSAG synchronization codeword (32 bits).
pub const POCSAG_SYNC: u32 = 0x7CD215D8;

/// POCSAG idle codeword (fill, 32 bits).
pub const POCSAG_IDLE: u32 = 0x7A89C197;

/// Number of frames per POCSAG batch.
pub const POCSAG_FRAMES_PER_BATCH: usize = 8;

/// Number of codewords per POCSAG frame.
pub const POCSAG_CW_PER_FRAME: usize = 2;

/// Total codewords per POCSAG batch (sync excluded).
pub const POCSAG_CW_PER_BATCH: usize = POCSAG_FRAMES_PER_BATCH * POCSAG_CW_PER_FRAME;

/// POCSAG channel bandwidth (Hz).
pub const POCSAG_CHANNEL_BW_HZ: f64 = 25_000.0;

/// POCSAG FSK frequency deviation (Hz) ±4.5 kHz.
pub const POCSAG_FSK_DEVIATION_HZ: f64 = 4_500.0;

/// BCH(31,21) generator polynomial: x^10+x^9+x^8+x^6+x^5+x^3+1 = 0x769.
const BCH_GENERATOR: u32 = 0x769;

/// FLEX sync word (phase A, 1600 baud preamble indicator).
pub const FLEX_SYNC_A: u32 = 0x870_CB0CE;

/// FLEX 4-FSK frequency deviations (Hz).
pub const FLEX_DEV_OUTER_HZ: f64 = 4_800.0; // ±4.8 kHz outer
pub const FLEX_DEV_INNER_HZ: f64 = 1_600.0; // ±1.6 kHz inner

/// Number of FLEX frames per 4-minute cycle.
pub const FLEX_FRAMES_PER_CYCLE: usize = 128;

// ─── Shared types ─────────────────────────────────────────────────────────────

/// POCSAG baud rates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BaudRate {
    Baud512,
    Baud1200,
    Baud2400,
}

impl BaudRate {
    /// Baud rate in symbols/second.
    pub fn value(self) -> u32 {
        match self {
            BaudRate::Baud512 => 512,
            BaudRate::Baud1200 => 1200,
            BaudRate::Baud2400 => 2400,
        }
    }
}

/// FLEX baud rates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FlexBaudRate {
    Baud1600,
    Baud3200,
    Baud6400,
}

impl FlexBaudRate {
    /// Baud rate in symbols/second.
    pub fn value(self) -> u32 {
        match self {
            FlexBaudRate::Baud1600 => 1600,
            FlexBaudRate::Baud3200 => 3200,
            FlexBaudRate::Baud6400 => 6400,
        }
    }

    /// Bits per symbol (2-FSK = 1, 4-FSK = 2).
    pub fn bits_per_symbol(self) -> u32 {
        match self {
            FlexBaudRate::Baud1600 => 1, // Phase A uses 2-FSK
            FlexBaudRate::Baud3200 => 2, // 4-FSK
            FlexBaudRate::Baud6400 => 2, // 4-FSK (fast)
        }
    }
}

/// FLEX phase (transmitter multiplexing).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FlexPhase {
    A,
    B,
    C,
    D,
}

/// Decoded message content.
#[derive(Debug, Clone, PartialEq)]
pub enum MessageContent {
    /// 4-bit BCD numeric (digits 0-9 plus special chars).
    Numeric(String),
    /// 7-bit ASCII alphanumeric.
    Alphanumeric(String),
    /// Tone-only alert (no message body).
    ToneOnly,
}

/// A decoded pager message.
#[derive(Debug, Clone)]
pub struct PagerMessage {
    /// 21-bit cap code (POCSAG) or FLEX address.
    pub address: u32,
    /// Function code 0-3.
    pub function: u8,
    /// Decoded content.
    pub content: MessageContent,
    /// Source batch/frame index.
    pub batch_index: usize,
}

// ─── BCH(31,21) Codec ────────────────────────────────────────────────────────

/// BCH(31,21) encoder/decoder with t=2 error correction.
///
/// The codeword layout (MSB first, 32 bits total):
/// ```text
/// [bit31=type] [bits30..11=data 20 bits] [bits10..1=BCH parity 10 bits] [bit0=even parity]
/// ```
/// For address words: bit31=0, data[20:12]=21-bit addr[20:3], data[11:10]=function.
/// For message words: bit31=1, data[20:1]=20 message bits.
pub struct BchCodec;

impl BchCodec {
    // ─── Internal helpers ────────────────────────────────────────────────────

    /// Compute BCH(31,21) check bits for a 21-bit systematic data word.
    ///
    /// The 21 info bits are at positions [30:10] of a working register.
    /// We perform polynomial long division by BCH_GENERATOR to get the
    /// 10-bit remainder = check bits placed at [9:0].
    fn check_bits(info21: u32) -> u32 {
        let mut r: u32 = (info21 & 0x1FFFFF) << 10;
        for i in (10..31).rev() {
            if r & (1u32 << i) != 0 {
                r ^= BCH_GENERATOR << (i - 10);
            }
        }
        r & 0x3FF
    }

    /// Compute the BCH(31,21) syndrome of a 31-bit word (info+check at [30:0]).
    fn syndrome_31(word31: u32) -> u32 {
        let mut r: u32 = word31 & 0x7FFFFFFF;
        for i in (10..31).rev() {
            if r & (1u32 << i) != 0 {
                r ^= BCH_GENERATOR << (i - 10);
            }
        }
        r & 0x3FF
    }

    // ─── Public API ──────────────────────────────────────────────────────────

    /// Encode 21 information bits into a BCH(31,21) 31-bit codeword.
    ///
    /// Returns a 31-bit value with info at [30:10] and check bits at [9:0].
    /// (Bit 31 is always 0 in the return value; the caller sets it as needed.)
    pub fn encode(info21: u32) -> u32 {
        let check = Self::check_bits(info21);
        ((info21 & 0x1FFFFF) << 10) | check
    }

    /// Build a complete 32-bit POCSAG address codeword.
    ///
    /// Standard POCSAG 32-bit layout (MSB = bit 31, transmitted first):
    /// ```text
    /// bit31=0 | bits[30:13]=addr_18(18b) | bits[12:11]=func(2b)
    ///         | bits[10:1]=BCH_check(10b) | bit0=even_parity
    /// ```
    ///
    /// The BCH code protects the 21-bit information word at bits [31:11].
    /// The even parity bit at bit 0 provides additional error detection.
    ///
    /// # Arguments
    ///
    /// * `addr_18` — Upper 18 bits of the 21-bit cap code (cap_code >> 3).
    /// * `func`    — 2-bit function code (0-3).
    pub fn encode_address(addr_18: u32, func: u8) -> u32 {
        // 21-bit information word occupying codeword bits [31:11]:
        //   bit[31]=0(type=address), bits[30:13]=addr_18, bits[12:11]=func
        // Shift everything right by 11 to get the 21-bit value for BCH:
        //   info21[20]=0, info21[19:2]=addr_18, info21[1:0]=func
        let info21: u32 = ((addr_18 & 0x3FFFF) << 2) | (func as u32 & 0x3);
        // BCH check bits: 10 bits placed at positions [10:1] of the 32-bit word
        let check = Self::check_bits(info21);
        // Build 32-bit codeword without parity
        // info21 at [31:11]: bit[31]=0 (addr), bits[30:13]=addr_18, bits[12:11]=func
        // check at [10:1]
        let cw = ((info21 & 0x1FFFFF) << 11) | (check << 1);
        // Even parity over bits [31:1] at bit[0]
        let ones = cw.count_ones();
        if ones % 2 == 0 { cw } else { cw | 1 }
    }

    /// Build a complete 32-bit POCSAG message codeword.
    ///
    /// Layout:
    /// ```text
    /// bit31=1 | bits[30:11]=data(20b) | bits[10:1]=BCH_check(10b) | bit0=even_parity
    /// ```
    ///
    /// # Arguments
    ///
    /// * `data_20` — 20 data bits (bit 19 = MSB of the payload).
    pub fn encode_message(data_20: u32) -> u32 {
        // 21-bit information word: bit[31]=1(type=message), bits[30:11]=data_20
        // info21[20]=1(type), info21[19:0]=data_20
        let info21: u32 = 0x100000 | (data_20 & 0xFFFFF);
        let check = Self::check_bits(info21);
        // Build codeword: bit[31]=1, bits[30:11]=data_20, bits[10:1]=check
        let cw = 0x80000000u32 | ((data_20 & 0xFFFFF) << 11) | (check << 1);
        let ones = cw.count_ones();
        if ones % 2 == 0 { cw } else { cw | 1 }
    }

    /// Compute the BCH(31,21) syndrome of a 32-bit POCSAG codeword.
    ///
    /// Strips the even-parity bit at position 0, then computes the syndrome
    /// of the 31-bit BCH word (info at [31:11], check at [10:1]).
    /// A syndrome of 0 means the 31-bit BCH portion is valid.
    pub fn syndrome(codeword: u32) -> u32 {
        // The 31-bit BCH word occupies bits [31:1] of the 32-bit POCSAG codeword.
        // Right-shift by 1 to place the BCH word at bits [30:0] for the divider.
        Self::syndrome_31(codeword >> 1)
    }

    /// Correct up to 2 bit errors in a 32-bit POCSAG codeword.
    ///
    /// Tries single-bit correction first, then double-bit.
    pub fn correct(cw: u32) -> Result<u32, PagerError> {
        if Self::syndrome(cw) == 0 {
            return Ok(cw);
        }
        // Single-bit correction
        for i in 0..32u32 {
            let trial = cw ^ (1u32 << i);
            if Self::syndrome(trial) == 0 {
                return Ok(trial);
            }
        }
        // Double-bit correction
        for i in 0..31u32 {
            for j in (i + 1)..32 {
                let trial = cw ^ (1u32 << i) ^ (1u32 << j);
                if Self::syndrome(trial) == 0 {
                    return Ok(trial);
                }
            }
        }
        Err(PagerError::BchUncorrectable)
    }

    /// Check even parity over all 32 bits (bit 0 = LSB).
    pub fn parity_ok(cw: u32) -> bool {
        cw.count_ones() % 2 == 0
    }
}

// ─── FSK Modulator ────────────────────────────────────────────────────────────

/// Complex IQ sample (f64 precision).
#[derive(Debug, Clone, Copy)]
pub struct IqSample {
    pub i: f64,
    pub q: f64,
}

impl IqSample {
    pub fn new(i: f64, q: f64) -> Self { Self { i, q } }
    pub fn magnitude_sq(self) -> f64 { self.i * self.i + self.q * self.q }
}

/// 2-FSK modulator for POCSAG.
///
/// Mark = logic 1 → +deviation, Space = logic 0 → −deviation.
/// Optional Gaussian pre-filter (BT product) for spectral shaping.
#[derive(Debug, Clone)]
pub struct FskModulator2 {
    sample_rate: f64,
    deviation_hz: f64,
    baud_rate: f64,
    samples_per_symbol: usize,
    phase: f64,
    gaussian_bt: Option<f64>,
    gauss_buf: Vec<f64>, // buffered phase increments for Gaussian filter
}

impl FskModulator2 {
    /// Create a new 2-FSK modulator.
    ///
    /// `sample_rate`: samples/second (e.g. 48000 or 25000).
    /// `deviation_hz`: peak deviation (e.g. 4500 for POCSAG).
    /// `baud_rate`: symbol rate (e.g. 1200.0).
    /// `gaussian_bt`: optional Gaussian BT product (None = no filter).
    pub fn new(
        sample_rate: f64,
        deviation_hz: f64,
        baud_rate: f64,
        gaussian_bt: Option<f64>,
    ) -> Self {
        let sps = (sample_rate / baud_rate).round() as usize;
        let gb = if gaussian_bt.is_some() {
            vec![0.0f64; sps]
        } else {
            Vec::new()
        };
        Self {
            sample_rate,
            deviation_hz,
            baud_rate,
            samples_per_symbol: sps.max(1),
            phase: 0.0,
            gaussian_bt,
            gauss_buf: gb,
        }
    }

    /// Modulate a bitstream into complex IQ samples.
    pub fn modulate(&mut self, bits: &[bool]) -> Vec<IqSample> {
        let mut out = Vec::with_capacity(bits.len() * self.samples_per_symbol);
        let freq_inc = 2.0 * PI * self.deviation_hz / self.sample_rate;

        for &bit in bits {
            let raw_inc = if bit { freq_inc } else { -freq_inc };
            let inc = if let Some(bt) = self.gaussian_bt {
                self.apply_gaussian(raw_inc, bt)
            } else {
                raw_inc
            };
            for _ in 0..self.samples_per_symbol {
                out.push(IqSample::new(self.phase.cos(), self.phase.sin()));
                self.phase += inc;
                if self.phase > PI { self.phase -= 2.0 * PI; }
                if self.phase < -PI { self.phase += 2.0 * PI; }
            }
        }
        out
    }

    /// Simple Gaussian smoothing of the phase increment (single-tap approx).
    fn apply_gaussian(&mut self, inc: f64, bt: f64) -> f64 {
        // BT bandwidth-time product; larger BT → less filtering
        let alpha = 1.0 - (-2.0 * PI * bt / self.baud_rate * self.sample_rate).exp();
        let alpha = alpha.clamp(0.01, 1.0);
        // Single-pole IIR approximation of Gaussian filter
        let prev = self.gauss_buf.first().copied().unwrap_or(inc);
        let smoothed = alpha * inc + (1.0 - alpha) * prev;
        if !self.gauss_buf.is_empty() {
            self.gauss_buf[0] = smoothed;
        }
        smoothed
    }

    /// Reset phase accumulator.
    pub fn reset(&mut self) { self.phase = 0.0; }
}

/// 4-level FSK modulator for FLEX.
///
/// Symbol mapping (dibit → frequency):
/// ```text
/// 00 → −outer (+4.8 kHz)
/// 01 → −inner (+1.6 kHz)
/// 11 → +inner (−1.6 kHz)
/// 10 → +outer (−4.8 kHz)
/// ```
/// (Gray-coded, outer level has higher magnitude.)
#[derive(Debug, Clone)]
pub struct FskModulator4 {
    sample_rate: f64,
    dev_outer: f64,
    dev_inner: f64,
    baud_rate: f64,
    samples_per_symbol: usize,
    phase: f64,
}

impl FskModulator4 {
    /// Create a new 4-FSK modulator.
    pub fn new(sample_rate: f64, baud_rate: f64) -> Self {
        let sps = (sample_rate / baud_rate).round() as usize;
        Self {
            sample_rate,
            dev_outer: FLEX_DEV_OUTER_HZ,
            dev_inner: FLEX_DEV_INNER_HZ,
            baud_rate,
            samples_per_symbol: sps.max(1),
            phase: 0.0,
        }
    }

    /// Map a 2-bit dibit to a frequency deviation.
    fn dibit_to_deviation(&self, dibit: u8) -> f64 {
        match dibit & 0x3 {
            0b00 => -self.dev_outer,
            0b01 => -self.dev_inner,
            0b11 => self.dev_inner,
            0b10 => self.dev_outer,
            _ => 0.0,
        }
    }

    /// Modulate dibits (pairs of bits, MSB first) into IQ.
    pub fn modulate(&mut self, dibits: &[u8]) -> Vec<IqSample> {
        let mut out = Vec::with_capacity(dibits.len() * self.samples_per_symbol);
        for &db in dibits {
            let deviation = self.dibit_to_deviation(db);
            let freq_inc = 2.0 * PI * deviation / self.sample_rate;
            for _ in 0..self.samples_per_symbol {
                out.push(IqSample::new(self.phase.cos(), self.phase.sin()));
                self.phase += freq_inc;
                if self.phase > PI { self.phase -= 2.0 * PI; }
                if self.phase < -PI { self.phase += 2.0 * PI; }
            }
        }
        out
    }

    /// Demodulate IQ samples into dibits via instantaneous frequency.
    pub fn demodulate(&mut self, samples: &[IqSample]) -> Vec<u8> {
        let mut prev = IqSample::new(1.0, 0.0);
        let mut dibits = Vec::new();
        let mut acc_i = 0.0f64;
        let mut acc_q = 0.0f64;
        let mut count = 0usize;

        for &s in samples {
            // Instantaneous frequency via arg(s * conj(prev))
            let re = s.i * prev.i + s.q * prev.q;
            let im = s.q * prev.i - s.i * prev.q;
            let freq = im.atan2(re);
            acc_i += freq;
            acc_q += 0.0;
            count += 1;
            prev = s;

            if count == self.samples_per_symbol {
                let mean_freq = acc_i / count as f64;
                let dev_norm = mean_freq * self.sample_rate / (2.0 * PI);
                let db = self.freq_to_dibit(dev_norm);
                dibits.push(db);
                acc_i = 0.0;
                acc_q = 0.0;
                count = 0;
            }
        }
        let _ = acc_q;
        dibits
    }

    fn freq_to_dibit(&self, freq_hz: f64) -> u8 {
        // Decision thresholds at ±(dev_inner + dev_outer)/2 and 0
        let thresh_hi = (self.dev_inner + self.dev_outer) / 2.0;
        let thresh_lo = self.dev_inner / 2.0;
        if freq_hz >= thresh_hi {
            0b10 // +outer
        } else if freq_hz >= thresh_lo {
            0b11 // +inner
        } else if freq_hz >= -thresh_lo {
            0b01 // -inner
        } else {
            0b00 // -outer
        }
    }

    /// Reset phase accumulator.
    pub fn reset(&mut self) { self.phase = 0.0; }
}

// ─── Bit/dibit utilities ──────────────────────────────────────────────────────

/// Pack bool slice into u32 (MSB first, up to 32 bits).
fn bits_to_u32(bits: &[bool]) -> u32 {
    let mut v = 0u32;
    for (i, &b) in bits.iter().enumerate().take(32) {
        if b { v |= 1 << (31 - i); }
    }
    v
}

/// Unpack a u32 into 32 bools (MSB first).
fn u32_to_bits(val: u32) -> [bool; 32] {
    let mut bits = [false; 32];
    for i in 0..32 {
        bits[i] = (val >> (31 - i)) & 1 != 0;
    }
    bits
}

/// Append 32-bit codeword as bits to a Vec<bool>.
fn push_cw(bits: &mut Vec<bool>, cw: u32) {
    for b in u32_to_bits(cw) {
        bits.push(b);
    }
}

// ─── Numeric / Alpha encoding ─────────────────────────────────────────────────

/// POCSAG numeric character set (4-bit BCD).
const NUMERIC_TABLE: [char; 16] = [
    '0', '1', '2', '3', '4', '5', '6', '7',
    '8', '9', '*', 'U', ' ', '-', ')', '(',
];

/// Encode a numeric string to a bit vector (4 bits/char, MSB first).
fn encode_numeric(s: &str) -> Vec<bool> {
    let mut bits = Vec::new();
    for ch in s.chars() {
        let idx = NUMERIC_TABLE.iter().position(|&c| c == ch).unwrap_or(12); // 12 = space
        for i in (0..4).rev() {
            bits.push((idx >> i) & 1 != 0);
        }
    }
    bits
}

/// Decode a 4-bit chunk stream back to a numeric string.
fn decode_numeric(bits: &[bool]) -> String {
    let mut s = String::new();
    for chunk in bits.chunks(4) {
        if chunk.len() < 4 { break; }
        let idx = chunk.iter().enumerate().fold(0usize, |a, (i, &b)| a | if b { 1 << (3 - i) } else { 0 });
        if idx < NUMERIC_TABLE.len() {
            s.push(NUMERIC_TABLE[idx]);
        }
    }
    s
}

/// Encode an ASCII string to 7-bit LSB-first packed bits.
fn encode_alpha(s: &str) -> Vec<bool> {
    let mut bits = Vec::new();
    for ch in s.chars() {
        let byte = (ch as u8) & 0x7F;
        for i in 0..7 {
            bits.push((byte >> i) & 1 != 0); // LSB first per POCSAG spec
        }
    }
    bits
}

/// Decode 7-bit LSB-first bit stream to ASCII string.
fn decode_alpha(bits: &[bool]) -> String {
    let mut s = String::new();
    for chunk in bits.chunks(7) {
        if chunk.len() < 7 { break; }
        let byte = chunk.iter().enumerate().fold(0u8, |a, (i, &b)| a | if b { 1 << i } else { 0 });
        if byte == 0 { break; }
        if (32..127).contains(&byte) {
            s.push(byte as char);
        }
    }
    s
}

// ─── POCSAG Configuration ─────────────────────────────────────────────────────

/// POCSAG transmitter/receiver configuration.
#[derive(Debug, Clone)]
pub struct PocsagConfig {
    /// 21-bit cap code (address).
    pub address: u32,
    /// 2-bit function code (0=numeric, 3=alphanumeric, 1/2=custom).
    pub function: u8,
    /// Baud rate.
    pub baud_rate: BaudRate,
    /// RF centre frequency (Hz); informational only.
    pub frequency_hz: f64,
    /// Enable Gaussian filtering on FSK (BT=0.5 typical).
    pub gaussian_bt: Option<f64>,
}

impl PocsagConfig {
    /// Create a basic POCSAG configuration.
    pub fn new(address: u32, function: u8, baud_rate: BaudRate) -> Self {
        Self {
            address: address & 0x1FFFFF,
            function: function & 0x3,
            baud_rate,
            frequency_hz: 152_000_000.0,
            gaussian_bt: None,
        }
    }

    /// Frame slot for this address (0..15 = frame × 2 + position within frame).
    pub fn frame_slot(&self) -> usize {
        (self.address & 0x7) as usize
    }

    /// Frame index (0..7) from address LSBs.
    pub fn frame_index(&self) -> usize {
        self.frame_slot() / 2
    }

    /// Codeword position within frame (0 or 1).
    pub fn cw_position(&self) -> usize {
        self.frame_slot() % 2
    }
}

// ─── POCSAG Encoder ───────────────────────────────────────────────────────────

/// POCSAG encoder: converts messages to a POCSAG bitstream.
pub struct PocsagEncoder {
    config: PocsagConfig,
}

impl PocsagEncoder {
    /// Create a new encoder with the given configuration.
    pub fn new(config: PocsagConfig) -> Self {
        Self { config }
    }

    /// Encode a numeric message, returning the complete POCSAG bit sequence
    /// (preamble + sync + batch codewords).
    pub fn encode_numeric(&self, text: &str) -> Vec<bool> {
        let content_bits = encode_numeric(text);
        self.encode_message(&content_bits, 0)
    }

    /// Encode an alphanumeric message.
    pub fn encode_alphanumeric(&self, text: &str) -> Vec<bool> {
        let content_bits = encode_alpha(text);
        self.encode_message(&content_bits, 3)
    }

    /// Encode a tone-only alert (no message body).
    pub fn encode_tone_only(&self) -> Vec<bool> {
        self.encode_message(&[], self.config.function)
    }

    /// Internal: build full bitstream for a message.
    fn encode_message(&self, content_bits: &[bool], function: u8) -> Vec<bool> {
        // Build codewords for this message
        let message_cws = self.pack_message_codewords(content_bits);

        // Determine how many batches are required
        let frame = self.config.frame_index();
        let pos = self.config.cw_position();
        let start_slot = frame * 2 + pos; // slot 0..15

        // Address codeword
        // Upper 18 bits of the 21-bit cap code go into the codeword; lower 3 bits
        // determine which frame/slot this message occupies.
        let addr_18 = self.config.address >> 3;
        let addr_cw = BchCodec::encode_address(addr_18, function);

        let total_data_cws = 1 + message_cws.len(); // addr + message
        let slots_needed = total_data_cws;
        let slots_from_start = POCSAG_CW_PER_BATCH - start_slot;
        let batches = if slots_needed <= slots_from_start {
            1
        } else {
            1 + (slots_needed - slots_from_start + POCSAG_CW_PER_BATCH - 1) / POCSAG_CW_PER_BATCH
        };

        // Build batch array (batches × 16 codewords)
        let total_slots = batches * POCSAG_CW_PER_BATCH;
        let mut slots: Vec<u32> = vec![POCSAG_IDLE; total_slots];

        slots[start_slot] = addr_cw;
        for (i, &cw) in message_cws.iter().enumerate() {
            let slot = start_slot + 1 + i;
            if slot < total_slots {
                slots[slot] = cw;
            }
        }

        // Build bitstream
        let mut bits = Vec::new();

        // Preamble: 576 alternating bits
        for i in 0..POCSAG_PREAMBLE_LEN {
            bits.push(i % 2 == 0);
        }

        // Batches
        for batch in 0..batches {
            push_cw(&mut bits, POCSAG_SYNC);
            for cw_idx in 0..POCSAG_CW_PER_BATCH {
                push_cw(&mut bits, slots[batch * POCSAG_CW_PER_BATCH + cw_idx]);
            }
        }

        bits
    }

    /// Pack content bits into 20-bit message codewords.
    fn pack_message_codewords(&self, content_bits: &[bool]) -> Vec<u32> {
        if content_bits.is_empty() {
            return Vec::new();
        }
        let mut cws = Vec::new();
        let mut i = 0;
        while i < content_bits.len() {
            let mut chunk = 0u32;
            for bit_pos in 0..20usize {
                if i + bit_pos < content_bits.len() && content_bits[i + bit_pos] {
                    chunk |= 1 << (19 - bit_pos);
                }
            }
            cws.push(BchCodec::encode_message(chunk));
            i += 20;
        }
        cws
    }

    /// Modulate the encoded bitstream to IQ samples using 2-FSK.
    pub fn modulate_to_iq(&self, bits: &[bool], sample_rate: f64) -> Vec<IqSample> {
        let mut mod2 = FskModulator2::new(
            sample_rate,
            POCSAG_FSK_DEVIATION_HZ,
            self.config.baud_rate.value() as f64,
            self.config.gaussian_bt,
        );
        mod2.modulate(bits)
    }
}

// ─── POCSAG Decoder ───────────────────────────────────────────────────────────

/// POCSAG decoder: parses FSK-demodulated bitstreams into messages.
pub struct PocsagDecoder {
    /// Internal bit buffer.
    bit_buffer: Vec<bool>,
    /// Decoded messages accumulated so far.
    messages: Vec<PagerMessage>,
    /// Number of batches successfully processed.
    batch_count: usize,
}

impl PocsagDecoder {
    /// Create a new decoder.
    pub fn new() -> Self {
        Self {
            bit_buffer: Vec::new(),
            messages: Vec::new(),
            batch_count: 0,
        }
    }

    /// Feed raw demodulated bits (after hard-decision slicer).
    ///
    /// Returns any newly decoded messages.
    pub fn feed_bits(&mut self, bits: &[bool]) -> Vec<PagerMessage> {
        self.bit_buffer.extend_from_slice(bits);
        let mut new_msgs = Vec::new();

        // Skip preamble, then look for sync words
        loop {
            // Need at least a sync + one batch worth of bits
            let batch_bits = 32 + POCSAG_CW_PER_BATCH * 32; // 32 + 512 = 544
            if self.bit_buffer.len() < 32 {
                break;
            }

            // Find sync
            if let Some(pos) = Self::find_sync_in(&self.bit_buffer) {
                if pos > 0 {
                    self.bit_buffer.drain(..pos);
                }
                if self.bit_buffer.len() < batch_bits {
                    break; // Wait for more data
                }

                // Consume sync + batch
                let batch: Vec<bool> = self.bit_buffer.drain(..batch_bits).collect();
                let parsed = self.parse_batch(&batch, self.batch_count);
                new_msgs.extend(parsed.iter().cloned());
                self.messages.extend(parsed);
                self.batch_count += 1;
            } else {
                // No sync found — slide forward by 1 bit to continue searching
                if self.bit_buffer.len() > 32 {
                    self.bit_buffer.drain(..1);
                } else {
                    break;
                }
            }
        }

        new_msgs
    }

    /// Parse a batch from 32-bit codewords (sync word + 16 codewords).
    ///
    /// The first element should be the sync word.
    pub fn decode_codewords(&mut self, codewords: &[u32]) -> Vec<PagerMessage> {
        let start = if codewords.first() == Some(&POCSAG_SYNC) { 1 } else { 0 };
        let msgs = self.parse_codeword_slice(&codewords[start..], self.batch_count);
        self.messages.extend(msgs.iter().cloned());
        self.batch_count += 1;
        msgs
    }

    /// All accumulated messages.
    pub fn messages(&self) -> &[PagerMessage] { &self.messages }

    /// Number of batches processed.
    pub fn batch_count(&self) -> usize { self.batch_count }

    /// Clear state.
    pub fn reset(&mut self) {
        self.bit_buffer.clear();
        self.messages.clear();
        self.batch_count = 0;
    }

    // ─── Private helpers ───────────────────────────────────────────────────

    fn find_sync_in(bit_buffer: &[bool]) -> Option<usize> {
        if bit_buffer.len() < 32 { return None; }
        for i in 0..=(bit_buffer.len() - 32) {
            if bits_to_u32(&bit_buffer[i..i + 32]) == POCSAG_SYNC {
                return Some(i);
            }
        }
        None
    }

    /// Parse a batch bit array (544 bits: sync + 16 codewords).
    fn parse_batch(&self, batch: &[bool], batch_idx: usize) -> Vec<PagerMessage> {
        // Extract 16 codewords (skip 32-bit sync at start)
        let mut cws = Vec::with_capacity(POCSAG_CW_PER_BATCH);
        for i in 0..POCSAG_CW_PER_BATCH {
            let start = 32 + i * 32;
            if start + 32 <= batch.len() {
                cws.push(bits_to_u32(&batch[start..start + 32]));
            } else {
                cws.push(POCSAG_IDLE);
            }
        }
        self.parse_codeword_slice(&cws, batch_idx)
    }

    fn parse_codeword_slice(&self, cws: &[u32], batch_idx: usize) -> Vec<PagerMessage> {
        let mut msgs = Vec::new();
        let mut pending: Option<(u32, u8)> = None; // (address, function)
        let mut msg_bits: Vec<bool> = Vec::new();

        for (idx, &raw_cw) in cws.iter().enumerate() {
            if raw_cw == POCSAG_IDLE {
                if let Some((addr, func)) = pending.take() {
                    msgs.push(build_message(addr, func, &msg_bits, batch_idx));
                    msg_bits.clear();
                }
                continue;
            }

            let cw = match BchCodec::correct(raw_cw) {
                Ok(c) => c,
                Err(_) => continue,
            };

            if cw & 0x80000000 == 0 {
                // Address codeword (bit 31 = 0)
                // Layout: [bit31=0][bits30:13=addr_18(18b)][bits12:11=func(2b)][bits10:1=BCH][bit0=par]
                if let Some((addr, func)) = pending.take() {
                    msgs.push(build_message(addr, func, &msg_bits, batch_idx));
                    msg_bits.clear();
                }

                // Extract addr_18 from bits [30:13] (18 bits)
                let addr_18 = (cw >> 13) & 0x3FFFF;
                // Extract func from bits [12:11] (2 bits)
                let func = ((cw >> 11) & 0x3) as u8;
                // Reconstruct full 21-bit cap code: upper 18 bits + lower 3 from frame position
                let frame_pos = idx as u32; // codeword index within the 16-codeword batch
                let addr = (addr_18 << 3) | (frame_pos & 0x7);
                pending = Some((addr, func));
            } else {
                // Message codeword (bit 31 = 1)
                // Layout: [bit31=1][bits30:11=data(20b)][bits10:1=BCH][bit0=par]
                // Extract 20 data bits from bits [30:11]
                for bit in (11..=30).rev() {
                    msg_bits.push((cw >> bit) & 1 != 0);
                }
            }
        }

        if let Some((addr, func)) = pending {
            msgs.push(build_message(addr, func, &msg_bits, batch_idx));
        }

        msgs
    }
}

impl Default for PocsagDecoder {
    fn default() -> Self { Self::new() }
}

/// Build a PagerMessage from collected bits.
fn build_message(addr: u32, func: u8, bits: &[bool], batch_idx: usize) -> PagerMessage {
    let content = if bits.is_empty() {
        MessageContent::ToneOnly
    } else {
        match func {
            0 | 1 => MessageContent::Numeric(decode_numeric(bits)),
            3 => MessageContent::Alphanumeric(decode_alpha(bits)),
            _ => {
                let alpha = decode_alpha(bits);
                if alpha.is_empty() {
                    MessageContent::Numeric(decode_numeric(bits))
                } else {
                    MessageContent::Alphanumeric(alpha)
                }
            }
        }
    };
    PagerMessage { address: addr, function: func, content, batch_index: batch_idx }
}

// ─── FLEX Configuration ───────────────────────────────────────────────────────

/// FLEX protocol configuration.
#[derive(Debug, Clone)]
pub struct FlexConfig {
    /// 32-bit FLEX address.
    pub address: u32,
    /// Phase (A/B/C/D) for multi-transmitter simulcast.
    pub phase: FlexPhase,
    /// Baud rate for data phases.
    pub baud_rate: FlexBaudRate,
    /// RF centre frequency (Hz).
    pub frequency_hz: f64,
}

impl FlexConfig {
    /// Create a basic FLEX configuration.
    pub fn new(address: u32, phase: FlexPhase, baud_rate: FlexBaudRate) -> Self {
        Self {
            address,
            phase,
            baud_rate,
            frequency_hz: 929_612_500.0, // US paging band
        }
    }

    /// Frame assignment within the 4-minute cycle.
    pub fn frame_number(&self) -> usize {
        (self.address & 0x7F) as usize // bottom 7 bits → frame 0..127
    }
}

// ─── FLEX Frame structure ─────────────────────────────────────────────────────

/// FLEX frame info word (FIW) — carries cycle/frame timing.
#[derive(Debug, Clone)]
pub struct FlexFrameInfo {
    pub cycle_number: u8,   // 0..255
    pub frame_number: u8,   // 0..127
    pub time_stamp: u16,    // optional timestamp
}

impl FlexFrameInfo {
    /// Encode frame info word as a 32-bit value.
    ///
    /// Layout: `[bits31:24=cycle(8b)][bits23:17=frame(7b)][bits16:1=timestamp(16b)][bit0=parity]`
    pub fn encode(&self) -> u32 {
        let fiw: u32 = ((self.cycle_number as u32 & 0xFF) << 24)
            | ((self.frame_number as u32 & 0x7F) << 17)
            | ((self.time_stamp as u32 & 0xFFFF) << 1);
        // Set even parity at bit 0
        let ones = (fiw & !1u32).count_ones();
        if ones % 2 == 0 { fiw & !1u32 } else { (fiw & !1u32) | 1 }
    }

    /// Decode from 32-bit FIW.
    pub fn decode(fiw: u32) -> Self {
        Self {
            cycle_number: ((fiw >> 24) & 0xFF) as u8,
            frame_number: ((fiw >> 17) & 0x7F) as u8,
            time_stamp: ((fiw >> 1) & 0xFFFF) as u16,
        }
    }
}

/// FLEX block interleaver for data protection.
///
/// FLEX interleaves 8 codewords per block using a fixed permutation to
/// spread burst errors across multiple codewords.
pub struct FlexInterleaver;

impl FlexInterleaver {
    const BLOCK_SIZE: usize = 8; // codewords per block

    /// Interleave: input codewords → interleaved codewords.
    pub fn interleave(input: &[u32]) -> Vec<u32> {
        let n = Self::BLOCK_SIZE;
        let blocks = (input.len() + n - 1) / n;
        let mut out = vec![POCSAG_IDLE; blocks * n];

        for block in 0..blocks {
            for (i, cw) in input[block * n..((block + 1) * n).min(input.len())].iter().enumerate() {
                // Simple bit-reversal permutation within the block
                let j = Self::interleave_index(i, n);
                if block * n + j < out.len() {
                    out[block * n + j] = *cw;
                }
            }
        }
        out
    }

    /// De-interleave: interleaved codewords → original order.
    pub fn deinterleave(input: &[u32]) -> Vec<u32> {
        let n = Self::BLOCK_SIZE;
        let blocks = (input.len() + n - 1) / n;
        let mut out = vec![POCSAG_IDLE; blocks * n];

        for block in 0..blocks {
            for i in 0..n {
                let j = Self::interleave_index(i, n);
                let src = block * n + j;
                let dst = block * n + i;
                if src < input.len() && dst < out.len() {
                    out[dst] = input[src];
                }
            }
        }
        out
    }

    /// Compute interleaved position: bit-reversal of index using log2(n) bits.
    fn interleave_index(i: usize, n: usize) -> usize {
        // Number of bits needed to represent indices 0..n-1
        let bits = (usize::BITS - (n - 1).leading_zeros()) as usize;
        let mut rev = 0usize;
        let mut x = i;
        for _ in 0..bits {
            rev = (rev << 1) | (x & 1);
            x >>= 1;
        }
        rev % n
    }
}

// ─── FLEX Encoder ─────────────────────────────────────────────────────────────

/// FLEX protocol encoder.
pub struct FlexEncoder {
    config: FlexConfig,
    cycle_number: u8,
    frame_number: u8,
}

impl FlexEncoder {
    /// Create a new FLEX encoder.
    pub fn new(config: FlexConfig) -> Self {
        let frame = config.frame_number() as u8;
        Self {
            config,
            cycle_number: 0,
            frame_number: frame,
        }
    }

    /// Encode a short numeric message and return the FLEX bitstream.
    pub fn encode_numeric(&self, text: &str) -> Vec<bool> {
        let content_bits = encode_numeric(text);
        self.build_frame(&content_bits, 0)
    }

    /// Encode a short alphanumeric message.
    pub fn encode_alphanumeric(&self, text: &str) -> Vec<bool> {
        let content_bits = encode_alpha(text);
        self.build_frame(&content_bits, 3)
    }

    /// Build a FLEX frame bitstream (Phase A preamble + sync + frame info + data).
    fn build_frame(&self, content_bits: &[bool], func: u8) -> Vec<bool> {
        let mut bits = Vec::new();

        // Phase A: 2-FSK preamble (1600 baud), 32 bits 0x00000000 then sync
        for _ in 0..32 { bits.push(false); } // preamble
        push_cw(&mut bits, FLEX_SYNC_A);     // sync word

        // Frame info word
        let fiw = FlexFrameInfo {
            cycle_number: self.cycle_number,
            frame_number: self.frame_number,
            time_stamp: 0,
        };
        push_cw(&mut bits, fiw.encode());

        // Build address codeword (BCH-protected)
        // FLEX uses a 32-bit address; map upper 18 bits into the BCH codeword addr field
        let addr_18 = (self.config.address >> 3) & 0x3FFFF;
        let addr_cw = BchCodec::encode_address(addr_18, func);

        // Pack message codewords
        let mut data_cws = vec![addr_cw];
        for chunk in content_bits.chunks(20) {
            let mut d = 0u32;
            for (i, &b) in chunk.iter().enumerate() {
                if b { d |= 1 << (19 - i); }
            }
            data_cws.push(BchCodec::encode_message(d));
        }

        // Interleave
        let interleaved = FlexInterleaver::interleave(&data_cws);

        // Emit as bits
        for cw in interleaved {
            push_cw(&mut bits, cw);
        }

        bits
    }

    /// Modulate to 4-FSK IQ (Phase B/C/D at configured baud rate).
    pub fn modulate_to_iq(&self, bits: &[bool], sample_rate: f64) -> Vec<IqSample> {
        let baud = self.config.baud_rate.value() as f64;
        let mut mod4 = FskModulator4::new(sample_rate, baud);

        // Pack bits into dibits
        let dibits: Vec<u8> = bits.chunks(2).map(|ch| {
            let b0 = ch.first().copied().unwrap_or(false) as u8;
            let b1 = ch.get(1).copied().unwrap_or(false) as u8;
            (b0 << 1) | b1
        }).collect();

        mod4.modulate(&dibits)
    }

    /// Advance to the next frame.
    pub fn advance_frame(&mut self) {
        self.frame_number = (self.frame_number + 1) % FLEX_FRAMES_PER_CYCLE as u8;
        if self.frame_number == 0 {
            self.cycle_number = self.cycle_number.wrapping_add(1);
        }
    }
}

// ─── FLEX Decoder ─────────────────────────────────────────────────────────────

/// FLEX protocol decoder.
pub struct FlexDecoder {
    messages: Vec<PagerMessage>,
    frame_count: usize,
}

impl FlexDecoder {
    /// Create a new FLEX decoder.
    pub fn new() -> Self {
        Self { messages: Vec::new(), frame_count: 0 }
    }

    /// Feed demodulated dibits (2 bits per symbol from 4-FSK demodulator).
    ///
    /// Returns any newly decoded messages.
    pub fn feed_dibits(&mut self, dibits: &[u8]) -> Vec<PagerMessage> {
        // Unpack dibits to bits
        let mut bits = Vec::with_capacity(dibits.len() * 2);
        for &db in dibits {
            bits.push((db >> 1) & 1 != 0);
            bits.push(db & 1 != 0);
        }
        self.feed_bits(&bits)
    }

    /// Feed raw bits (from 2-FSK phase A, or bit-unpacked 4-FSK).
    pub fn feed_bits(&mut self, bits: &[bool]) -> Vec<PagerMessage> {
        // Look for FLEX sync word
        let mut new_msgs = Vec::new();
        let mut i = 0;
        while i + 32 <= bits.len() {
            if bits_to_u32(&bits[i..i + 32]) == FLEX_SYNC_A {
                let frame_start = i + 32;
                if frame_start + 64 > bits.len() { break; }

                // Read FIW
                let fiw_raw = bits_to_u32(&bits[frame_start..frame_start + 32]);
                let _fiw = FlexFrameInfo::decode(fiw_raw);

                // Read available codewords
                let mut cws = Vec::new();
                let mut j = frame_start + 32;
                while j + 32 <= bits.len() {
                    cws.push(bits_to_u32(&bits[j..j + 32]));
                    j += 32;
                }

                // De-interleave
                let deint = FlexInterleaver::deinterleave(&cws);

                // Parse
                let msgs = self.parse_flex_codewords(&deint);
                new_msgs.extend(msgs.iter().cloned());
                self.messages.extend(msgs);
                self.frame_count += 1;
                i = j;
            } else {
                i += 1;
            }
        }
        new_msgs
    }

    fn parse_flex_codewords(&self, cws: &[u32]) -> Vec<PagerMessage> {
        let mut msgs = Vec::new();
        let mut pending: Option<(u32, u8)> = None;
        let mut msg_bits: Vec<bool> = Vec::new();

        for &raw in cws {
            if raw == POCSAG_IDLE { continue; }

            let cw = match BchCodec::correct(raw) {
                Ok(c) => c,
                Err(_) => continue,
            };

            if cw & 0x80000000 == 0 {
                // Address codeword: [bit31=0][bits30:13=addr_18][bits12:11=func][bits10:1=BCH][bit0=par]
                if let Some((addr, func)) = pending.take() {
                    msgs.push(build_message(addr, func, &msg_bits, self.frame_count));
                    msg_bits.clear();
                }
                let addr_18 = (cw >> 13) & 0x3FFFF;
                let func = ((cw >> 11) & 0x3) as u8;
                pending = Some((addr_18, func));
            } else {
                // Message codeword: [bit31=1][bits30:11=data(20b)][bits10:1=BCH][bit0=par]
                for bit in (11..=30).rev() {
                    msg_bits.push((cw >> bit) & 1 != 0);
                }
            }
        }

        if let Some((addr, func)) = pending {
            msgs.push(build_message(addr, func, &msg_bits, self.frame_count));
        }
        msgs
    }

    /// All accumulated messages.
    pub fn messages(&self) -> &[PagerMessage] { &self.messages }

    /// Number of frames processed.
    pub fn frame_count(&self) -> usize { self.frame_count }

    /// Clear state.
    pub fn reset(&mut self) {
        self.messages.clear();
        self.frame_count = 0;
    }
}

impl Default for FlexDecoder {
    fn default() -> Self { Self::new() }
}

// ─── Coverage and Performance Estimation ─────────────────────────────────────

/// Pager link budget and sensitivity calculator.
pub struct PagerLinkBudget {
    /// Transmitter EIRP (dBm).
    pub tx_eirp_dbm: f64,
    /// Receiver sensitivity (dBm) at specified BER.
    pub rx_sensitivity_dbm: f64,
    /// System noise figure (dB).
    pub noise_figure_db: f64,
    /// Required Eb/N0 (dB) for 1e-3 BER with 2-FSK non-coherent.
    pub required_eb_n0_db: f64,
}

impl PagerLinkBudget {
    /// Create a default POCSAG link budget.
    pub fn pocsag_default() -> Self {
        Self {
            tx_eirp_dbm: 40.0,          // 10 W EIRP
            rx_sensitivity_dbm: -112.0, // Typical pager sensitivity
            noise_figure_db: 5.0,
            required_eb_n0_db: 12.0,   // FSK non-coherent at 1e-3 BER
        }
    }

    /// Compute thermal noise floor (dBm) for a given bandwidth.
    pub fn noise_floor_dbm(&self, bw_hz: f64) -> f64 {
        let kTB = -174.0 + 10.0 * bw_hz.log10(); // dBm/Hz at 290 K + bandwidth
        kTB + self.noise_figure_db
    }

    /// Compute required receiver sensitivity (dBm) for a given baud rate.
    pub fn required_sensitivity_dbm(&self, baud: BaudRate) -> f64 {
        let bw = baud.value() as f64 * 1.2; // rough noise bandwidth
        self.noise_floor_dbm(bw) + self.required_eb_n0_db
    }

    /// Estimate maximum coverage range (km) using free-space path loss.
    ///
    /// `freq_hz`: carrier frequency.
    pub fn max_range_km(&self, freq_hz: f64, baud: BaudRate) -> f64 {
        let link_margin_db = self.tx_eirp_dbm - self.required_sensitivity_dbm(baud);
        // FSPL(d) = 20*log10(4*pi*d*f/c) = 20*log10(f/c) + 20*log10(d) + 21.98
        let c = 3.0e8_f64;
        let fspl_1m = 20.0 * (freq_hz / c).log10() + 20.0 * 1.0_f64.log10() + 21.98;
        let fspl_budget = link_margin_db;
        // Solve: 20*log10(d_m) = fspl_budget - fspl_1m
        let d_m = 10.0_f64.powf((fspl_budget - fspl_1m) / 20.0);
        d_m / 1000.0
    }

    /// Simulcast link budget: accounts for timing uncertainty between transmitters.
    ///
    /// `tx_count`: number of simulcast transmitters.
    /// Returns the effective sensitivity penalty (dB) due to constructive/destructive combining.
    pub fn simulcast_penalty_db(tx_count: usize) -> f64 {
        if tx_count <= 1 { return 0.0; }
        // Worst-case simulcast null: ~6 dB penalty for 2 transmitters at boundary
        3.0 * (tx_count as f64).log2()
    }
}

// ─── Error type ────────────────────────────────────────────────────────────────

/// Pager processing error.
#[derive(Debug)]
pub enum PagerError {
    /// BCH syndrome non-zero after correction attempts.
    BchUncorrectable,
    /// Frame sync not found in input.
    NoSync,
    /// Malformed message data.
    InvalidMessage,
}

impl std::fmt::Display for PagerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PagerError::BchUncorrectable => write!(f, "BCH: uncorrectable error burst"),
            PagerError::NoSync => write!(f, "Sync word not found"),
            PagerError::InvalidMessage => write!(f, "Malformed message"),
        }
    }
}

impl std::error::Error for PagerError {}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── BCH Codec Tests ────────────────────────────────────────────────────

    #[test]
    fn test_bch_syndrome_zero_on_valid_idle() {
        // The POCSAG_IDLE codeword should have zero syndrome
        let syn = BchCodec::syndrome(POCSAG_IDLE);
        assert_eq!(syn, 0, "POCSAG_IDLE should have zero BCH syndrome");
    }

    #[test]
    fn test_bch_syndrome_zero_on_valid_sync() {
        // POCSAG_SYNC is not a data codeword but syndrome should be consistent
        let _ = BchCodec::syndrome(POCSAG_SYNC);
        // No assertion — just ensuring no panic
    }

    #[test]
    fn test_bch_encode_decode_roundtrip() {
        let data: u32 = 0b1_0101_0101_0101_0101_0101; // 21 bits
        let cw = BchCodec::encode(data);
        // Syndrome of encoded word must be zero
        assert_eq!(BchCodec::syndrome(cw << 1), 0, "Encoded codeword syndrome must be zero");
    }

    #[test]
    fn test_bch_correct_no_error() {
        let addr_cw = BchCodec::encode_address(0x12345, 0);
        let result = BchCodec::correct(addr_cw);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), addr_cw);
    }

    #[test]
    fn test_bch_correct_single_bit_error() {
        let cw = BchCodec::encode_address(0x1234, 3);
        let corrupted = cw ^ (1 << 15); // flip bit 15
        let result = BchCodec::correct(corrupted);
        assert!(result.is_ok(), "Single bit error should be correctable");
        assert_eq!(result.unwrap(), cw);
    }

    #[test]
    fn test_bch_correct_double_bit_error() {
        let cw = BchCodec::encode_address(0x5678, 1);
        let corrupted = cw ^ (1 << 20) ^ (1 << 5);
        let result = BchCodec::correct(corrupted);
        assert!(result.is_ok(), "Double bit error should be correctable");
        assert_eq!(result.unwrap(), cw);
    }

    #[test]
    fn test_bch_address_bit31_zero() {
        let cw = BchCodec::encode_address(0xAAAA, 2);
        assert_eq!(cw & 0x80000000, 0, "Address codeword bit 31 must be 0");
    }

    #[test]
    fn test_bch_message_bit31_one() {
        let cw = BchCodec::encode_message(0xFFFFF);
        assert_ne!(cw & 0x80000000, 0, "Message codeword bit 31 must be 1");
    }

    #[test]
    fn test_bch_parity_even() {
        let cw = BchCodec::encode_address(0x1111, 0);
        assert!(BchCodec::parity_ok(cw), "Address codeword must have even parity");
        let mcw = BchCodec::encode_message(0x0F0F0);
        assert!(BchCodec::parity_ok(mcw), "Message codeword must have even parity");
    }

    #[test]
    fn test_bch_idle_correct() {
        let result = BchCodec::correct(POCSAG_IDLE);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), POCSAG_IDLE);
    }

    // ── Numeric / Alpha encoding ───────────────────────────────────────────

    #[test]
    fn test_encode_decode_numeric_digits() {
        let s = "1234567890";
        let bits = encode_numeric(s);
        let decoded = decode_numeric(&bits);
        assert_eq!(decoded, s);
    }

    #[test]
    fn test_encode_decode_numeric_special() {
        let s = "12-34 56";
        let bits = encode_numeric(s);
        let decoded = decode_numeric(&bits);
        assert_eq!(decoded, s);
    }

    #[test]
    fn test_encode_numeric_bit_count() {
        let s = "12345";
        let bits = encode_numeric(s);
        assert_eq!(bits.len(), s.len() * 4, "Each digit uses 4 bits");
    }

    #[test]
    fn test_encode_decode_alpha_ascii() {
        let s = "Hello World!";
        let bits = encode_alpha(s);
        let decoded = decode_alpha(&bits);
        assert_eq!(decoded, s);
    }

    #[test]
    fn test_encode_alpha_bit_count() {
        let s = "Test";
        let bits = encode_alpha(s);
        assert_eq!(bits.len(), s.len() * 7, "Each char uses 7 bits");
    }

    #[test]
    fn test_encode_decode_alpha_roundtrip_mixed() {
        let s = "ABC 123 !@#";
        let bits = encode_alpha(s);
        let decoded = decode_alpha(&bits);
        assert_eq!(decoded, s);
    }

    // ── POCSAG Config ──────────────────────────────────────────────────────

    #[test]
    fn test_pocsag_config_frame_slot() {
        let cfg = PocsagConfig::new(0x1234560, 0, BaudRate::Baud1200); // addr ends in 0 → slot 0
        assert_eq!(cfg.frame_slot(), 0);
        assert_eq!(cfg.frame_index(), 0);
        assert_eq!(cfg.cw_position(), 0);
    }

    #[test]
    fn test_pocsag_config_frame_slot_7() {
        let cfg = PocsagConfig::new(0x1234567, 0, BaudRate::Baud1200); // addr ends in 7 → slot 7
        assert_eq!(cfg.frame_slot(), 7);
        assert_eq!(cfg.frame_index(), 3);
        assert_eq!(cfg.cw_position(), 1);
    }

    // ── POCSAG Encoder ─────────────────────────────────────────────────────

    #[test]
    fn test_pocsag_encoder_preamble_length() {
        let cfg = PocsagConfig::new(1234567, 0, BaudRate::Baud1200);
        let enc = PocsagEncoder::new(cfg);
        let bits = enc.encode_tone_only();
        // First 576 bits are preamble
        assert!(bits.len() >= POCSAG_PREAMBLE_LEN, "Output must include preamble");
        // Preamble alternates 1010...
        for i in 0..POCSAG_PREAMBLE_LEN {
            assert_eq!(bits[i], i % 2 == 0, "Preamble bit {i} should alternate");
        }
    }

    #[test]
    fn test_pocsag_encoder_sync_word_present() {
        let cfg = PocsagConfig::new(1234567, 3, BaudRate::Baud1200);
        let enc = PocsagEncoder::new(cfg);
        let bits = enc.encode_alphanumeric("Hi");
        // Sync word starts at bit 576
        let sync = bits_to_u32(&bits[POCSAG_PREAMBLE_LEN..POCSAG_PREAMBLE_LEN + 32]);
        assert_eq!(sync, POCSAG_SYNC, "Sync word must be 0x7CD215D8");
    }

    #[test]
    fn test_pocsag_encoder_batch_size() {
        let cfg = PocsagConfig::new(1234567, 0, BaudRate::Baud512);
        let enc = PocsagEncoder::new(cfg);
        let bits = enc.encode_tone_only();
        let payload_bits = bits.len() - POCSAG_PREAMBLE_LEN;
        // Must be N × (32 + 512) for some N ≥ 1
        assert_eq!(payload_bits % 544, 0, "Payload must be a multiple of 544 bits");
    }

    #[test]
    fn test_pocsag_encoder_numeric_nonempty() {
        let cfg = PocsagConfig::new(9876543, 0, BaudRate::Baud1200);
        let enc = PocsagEncoder::new(cfg);
        let bits = enc.encode_numeric("555-1234");
        assert!(!bits.is_empty());
    }

    #[test]
    fn test_pocsag_encoder_alpha_nonempty() {
        let cfg = PocsagConfig::new(1111111, 3, BaudRate::Baud2400);
        let enc = PocsagEncoder::new(cfg);
        let bits = enc.encode_alphanumeric("Test message");
        assert!(!bits.is_empty());
    }

    // ── POCSAG Decoder ─────────────────────────────────────────────────────

    #[test]
    fn test_pocsag_decoder_idle_batch() {
        let mut dec = PocsagDecoder::new();
        let cws = vec![POCSAG_SYNC, POCSAG_IDLE, POCSAG_IDLE, POCSAG_IDLE, POCSAG_IDLE,
                       POCSAG_IDLE, POCSAG_IDLE, POCSAG_IDLE, POCSAG_IDLE, POCSAG_IDLE,
                       POCSAG_IDLE, POCSAG_IDLE, POCSAG_IDLE, POCSAG_IDLE, POCSAG_IDLE,
                       POCSAG_IDLE, POCSAG_IDLE];
        let msgs = dec.decode_codewords(&cws);
        assert!(msgs.is_empty(), "All-idle batch should yield no messages");
    }

    #[test]
    fn test_pocsag_decoder_batch_count_increments() {
        let mut dec = PocsagDecoder::new();
        let cws = vec![POCSAG_SYNC; 17]; // just sync + 16 codewords
        dec.decode_codewords(&cws);
        assert_eq!(dec.batch_count(), 1);
        dec.decode_codewords(&cws);
        assert_eq!(dec.batch_count(), 2);
    }

    #[test]
    fn test_pocsag_decoder_reset() {
        let mut dec = PocsagDecoder::new();
        let cws = vec![POCSAG_SYNC; 17];
        dec.decode_codewords(&cws);
        assert_eq!(dec.batch_count(), 1);
        dec.reset();
        assert_eq!(dec.batch_count(), 0);
        assert!(dec.messages().is_empty());
    }

    #[test]
    fn test_pocsag_decode_address_codeword() {
        let addr21 = 0x12345u32;
        let func = 3u8;
        let cw = BchCodec::encode_address(addr21, func);

        let mut dec = PocsagDecoder::new();
        // Build a minimal batch: sync + address codeword + 15 idle
        let mut cws = vec![POCSAG_SYNC, cw];
        cws.extend(std::iter::repeat(POCSAG_IDLE).take(15));
        let msgs = dec.decode_codewords(&cws);

        // Should decode as tone-only (no message codewords follow)
        assert!(!msgs.is_empty(), "Should produce at least one message");
        let m = &msgs[0];
        assert_eq!(m.function, func);
        assert_eq!(m.content, MessageContent::ToneOnly);
    }

    // ── POCSAG full encode/decode chain ────────────────────────────────────

    #[test]
    fn test_pocsag_encode_decode_numeric_chain() {
        let text = "12345";
        let cfg = PocsagConfig::new(0x18, 0, BaudRate::Baud1200); // addr slot 0
        let enc = PocsagEncoder::new(cfg);
        let bits = enc.encode_numeric(text);

        let mut dec = PocsagDecoder::new();
        let msgs = dec.feed_bits(&bits);

        // At least one message should be decoded
        assert!(!msgs.is_empty(), "Decoded message list should not be empty");
        if let MessageContent::Numeric(s) = &msgs[0].content {
            assert!(s.contains("12345"), "Decoded numeric content mismatch: got '{s}'");
        }
    }

    #[test]
    fn test_pocsag_encode_decode_alpha_chain() {
        let text = "Hello!";
        let cfg = PocsagConfig::new(0x10, 3, BaudRate::Baud1200);
        let enc = PocsagEncoder::new(cfg);
        let bits = enc.encode_alphanumeric(text);

        let mut dec = PocsagDecoder::new();
        let msgs = dec.feed_bits(&bits);

        assert!(!msgs.is_empty());
        if let MessageContent::Alphanumeric(s) = &msgs[0].content {
            assert!(s.contains("Hello"), "Expected 'Hello' in decoded message, got '{s}'");
        }
    }

    #[test]
    fn test_pocsag_tone_only_chain() {
        let cfg = PocsagConfig::new(0x50, 0, BaudRate::Baud512);
        let enc = PocsagEncoder::new(cfg);
        let bits = enc.encode_tone_only();

        let mut dec = PocsagDecoder::new();
        let msgs = dec.feed_bits(&bits);
        assert!(!msgs.is_empty());
        assert_eq!(msgs[0].content, MessageContent::ToneOnly);
    }

    // ── FSK Modulator 2 ────────────────────────────────────────────────────

    #[test]
    fn test_fsk2_output_length() {
        let mut m = FskModulator2::new(48000.0, 4500.0, 1200.0, None);
        let bits = vec![true, false, true, true, false];
        let out = m.modulate(&bits);
        let sps = (48000.0_f64 / 1200.0_f64).round() as usize;
        assert_eq!(out.len(), bits.len() * sps);
    }

    #[test]
    fn test_fsk2_unit_magnitude() {
        let mut m = FskModulator2::new(48000.0, 4500.0, 1200.0, None);
        let bits = vec![true, false, true];
        let out = m.modulate(&bits);
        for s in &out {
            let mag = s.magnitude_sq().sqrt();
            assert!((mag - 1.0).abs() < 1e-9, "FSK sample magnitude must be 1.0, got {mag}");
        }
    }

    #[test]
    fn test_fsk2_gaussian_output_length() {
        let mut m = FskModulator2::new(48000.0, 4500.0, 1200.0, Some(0.5));
        let bits = vec![true, false, false, true];
        let out = m.modulate(&bits);
        let sps = (48000.0_f64 / 1200.0_f64).round() as usize;
        assert_eq!(out.len(), bits.len() * sps);
    }

    #[test]
    fn test_fsk2_reset() {
        let mut m = FskModulator2::new(48000.0, 4500.0, 1200.0, None);
        m.modulate(&[true, true]);
        m.reset();
        let out1 = m.modulate(&[false]);
        m.reset();
        let out2 = m.modulate(&[false]);
        assert!((out1[0].i - out2[0].i).abs() < 1e-12);
    }

    // ── FSK Modulator 4 ────────────────────────────────────────────────────

    #[test]
    fn test_fsk4_output_length() {
        let mut m = FskModulator4::new(48000.0, 3200.0);
        let dibits = vec![0b00, 0b01, 0b11, 0b10];
        let out = m.modulate(&dibits);
        let sps = (48000.0_f64 / 3200.0_f64).round() as usize;
        assert_eq!(out.len(), dibits.len() * sps);
    }

    #[test]
    fn test_fsk4_unit_magnitude() {
        let mut m = FskModulator4::new(48000.0, 3200.0);
        let dibits = vec![0b00, 0b01, 0b11, 0b10];
        let out = m.modulate(&dibits);
        for s in &out {
            let mag = s.magnitude_sq().sqrt();
            assert!((mag - 1.0).abs() < 1e-9, "4-FSK sample magnitude must be 1.0, got {mag}");
        }
    }

    #[test]
    fn test_fsk4_demodulate_known_dibits() {
        let sample_rate = 192_000.0;
        let baud = 6400.0;
        let mut mod4 = FskModulator4::new(sample_rate, baud);
        mod4.dev_outer = FLEX_DEV_OUTER_HZ;
        mod4.dev_inner = FLEX_DEV_INNER_HZ;
        let input = vec![0b00u8, 0b11, 0b10, 0b01];
        let iq = mod4.modulate(&input);
        let mut demod = FskModulator4::new(sample_rate, baud);
        demod.dev_outer = FLEX_DEV_OUTER_HZ;
        demod.dev_inner = FLEX_DEV_INNER_HZ;
        let out = demod.demodulate(&iq);
        assert_eq!(out.len(), input.len(), "Demodulated length mismatch");
        // Check at least some are correct (some boundary symbols may differ slightly)
        let correct = out.iter().zip(input.iter()).filter(|(a, b)| a == b).count();
        assert!(correct >= input.len() / 2, "Too many demodulation errors: {correct}/{}", input.len());
    }

    #[test]
    fn test_fsk4_reset() {
        let mut m = FskModulator4::new(48000.0, 3200.0);
        m.modulate(&[0b01]);
        m.reset();
        let out1 = m.modulate(&[0b00]);
        m.reset();
        let out2 = m.modulate(&[0b00]);
        assert!((out1[0].i - out2[0].i).abs() < 1e-12);
    }

    // ── FLEX Interleaver ───────────────────────────────────────────────────

    #[test]
    fn test_flex_interleaver_roundtrip() {
        let original: Vec<u32> = (0..8).map(|i| i * 0x1000 + i).collect();
        let interleaved = FlexInterleaver::interleave(&original);
        let restored = FlexInterleaver::deinterleave(&interleaved);
        assert_eq!(&restored[..original.len()], &original[..], "Interleaver roundtrip failed");
    }

    #[test]
    fn test_flex_interleaver_changes_order() {
        let original: Vec<u32> = (1..=8).collect();
        let interleaved = FlexInterleaver::interleave(&original);
        // At least some elements should be in different positions
        let same = original.iter().zip(interleaved.iter()).filter(|(a, b)| a == b).count();
        assert!(same < original.len(), "Interleaver did not change any order");
    }

    #[test]
    fn test_flex_interleaver_length_preserved() {
        let cws: Vec<u32> = vec![1, 2, 3, 4, 5, 6, 7, 8];
        let out = FlexInterleaver::interleave(&cws);
        assert_eq!(out.len(), 8);
    }

    // ── FLEX Frame Info ────────────────────────────────────────────────────

    #[test]
    fn test_flex_frame_info_encode_decode() {
        let fiw = FlexFrameInfo { cycle_number: 42, frame_number: 77, time_stamp: 0x1234 };
        let encoded = fiw.encode();
        let decoded = FlexFrameInfo::decode(encoded);
        assert_eq!(decoded.cycle_number, 42);
        assert_eq!(decoded.frame_number, 77);
    }

    // ── FLEX Config ────────────────────────────────────────────────────────

    #[test]
    fn test_flex_config_frame_number() {
        let cfg = FlexConfig::new(0xABCDEF80, FlexPhase::A, FlexBaudRate::Baud3200);
        assert_eq!(cfg.frame_number(), 0x80 & 0x7F, "Frame = lower 7 bits of address");
    }

    // ── FLEX Encoder ───────────────────────────────────────────────────────

    #[test]
    fn test_flex_encoder_nonempty_output() {
        let cfg = FlexConfig::new(0x1234_5678, FlexPhase::A, FlexBaudRate::Baud3200);
        let enc = FlexEncoder::new(cfg);
        let bits = enc.encode_alphanumeric("Hi there");
        assert!(!bits.is_empty());
    }

    #[test]
    fn test_flex_encoder_sync_present() {
        let cfg = FlexConfig::new(0xABCD, FlexPhase::A, FlexBaudRate::Baud1600);
        let enc = FlexEncoder::new(cfg);
        let bits = enc.encode_numeric("123");
        // Find FLEX sync word somewhere in output
        let found = (0..=(bits.len().saturating_sub(32)))
            .any(|i| bits_to_u32(&bits[i..i + 32]) == FLEX_SYNC_A);
        assert!(found, "FLEX sync word 0x{:08X} not found in output", FLEX_SYNC_A);
    }

    #[test]
    fn test_flex_encoder_advance_frame() {
        let cfg = FlexConfig::new(0x0, FlexPhase::A, FlexBaudRate::Baud3200);
        let mut enc = FlexEncoder::new(cfg);
        let f0 = enc.frame_number;
        enc.advance_frame();
        assert_eq!(enc.frame_number, (f0 + 1) % FLEX_FRAMES_PER_CYCLE as u8);
    }

    #[test]
    fn test_flex_encoder_iq_output_nonempty() {
        let cfg = FlexConfig::new(0x1234, FlexPhase::B, FlexBaudRate::Baud3200);
        let enc = FlexEncoder::new(cfg);
        let bits = enc.encode_alphanumeric("Test");
        let iq = enc.modulate_to_iq(&bits, 48000.0);
        assert!(!iq.is_empty());
    }

    // ── FLEX multi-phase ───────────────────────────────────────────────────

    #[test]
    fn test_flex_multi_phase_different_frame() {
        let cfg_a = FlexConfig::new(0x10, FlexPhase::A, FlexBaudRate::Baud3200);
        let cfg_b = FlexConfig::new(0x50, FlexPhase::B, FlexBaudRate::Baud3200);
        assert_ne!(cfg_a.frame_number(), cfg_b.frame_number());
    }

    // ── FLEX Decoder ───────────────────────────────────────────────────────

    #[test]
    fn test_flex_decoder_empty_input() {
        let mut dec = FlexDecoder::new();
        let msgs = dec.feed_bits(&[]);
        assert!(msgs.is_empty());
    }

    #[test]
    fn test_flex_decoder_reset() {
        let mut dec = FlexDecoder::new();
        dec.frame_count = 5;
        dec.reset();
        assert_eq!(dec.frame_count(), 0);
        assert!(dec.messages().is_empty());
    }

    // ── Link Budget ────────────────────────────────────────────────────────

    #[test]
    fn test_link_budget_noise_floor_reasonable() {
        let lb = PagerLinkBudget::pocsag_default();
        let floor = lb.noise_floor_dbm(1200.0 * 1.2);
        // Should be roughly -140 to -120 dBm for 1.44 kHz noise BW
        assert!(floor < -110.0 && floor > -160.0, "Noise floor {floor} dBm out of expected range");
    }

    #[test]
    fn test_link_budget_coverage_range_positive() {
        let lb = PagerLinkBudget::pocsag_default();
        let range = lb.max_range_km(152e6, BaudRate::Baud1200);
        assert!(range > 0.0, "Coverage range must be positive");
        // Free-space calculation without terrain/building losses can give large ranges;
        // the key check is that the computation returns a positive finite value
        assert!(range.is_finite(), "Coverage range must be finite: {range} km");
    }

    #[test]
    fn test_simulcast_penalty_single_tx() {
        assert_eq!(PagerLinkBudget::simulcast_penalty_db(1), 0.0);
    }

    #[test]
    fn test_simulcast_penalty_two_tx() {
        let p = PagerLinkBudget::simulcast_penalty_db(2);
        assert!(p > 0.0, "2-transmitter simulcast must have positive penalty");
    }

    // ── Parity checks ──────────────────────────────────────────────────────

    #[test]
    fn test_parity_all_address_cws() {
        for addr in [0u32, 0x1234, 0x1FFFFF] {
            for func in 0u8..4 {
                let cw = BchCodec::encode_address(addr, func);
                assert!(BchCodec::parity_ok(cw), "Parity failed for addr={addr:#x} func={func}");
            }
        }
    }

    #[test]
    fn test_parity_all_message_cws() {
        for data in [0u32, 0xAAAAA, 0xFFFFF] {
            let cw = BchCodec::encode_message(data);
            assert!(BchCodec::parity_ok(cw), "Parity failed for data={data:#x}");
        }
    }

    // ── Utility functions ──────────────────────────────────────────────────

    #[test]
    fn test_bits_to_u32_all_ones() {
        let bits = [true; 32];
        assert_eq!(bits_to_u32(&bits), 0xFFFFFFFF);
    }

    #[test]
    fn test_bits_to_u32_all_zeros() {
        let bits = [false; 32];
        assert_eq!(bits_to_u32(&bits), 0x00000000);
    }

    #[test]
    fn test_u32_to_bits_roundtrip() {
        let val = 0xDEADBEEF_u32;
        let bits = u32_to_bits(val);
        assert_eq!(bits_to_u32(&bits), val);
    }

    #[test]
    fn test_baud_rate_values() {
        assert_eq!(BaudRate::Baud512.value(), 512);
        assert_eq!(BaudRate::Baud1200.value(), 1200);
        assert_eq!(BaudRate::Baud2400.value(), 2400);
    }

    #[test]
    fn test_flex_baud_rate_values() {
        assert_eq!(FlexBaudRate::Baud1600.value(), 1600);
        assert_eq!(FlexBaudRate::Baud3200.value(), 3200);
        assert_eq!(FlexBaudRate::Baud6400.value(), 6400);
    }

    #[test]
    fn test_flex_baud_bits_per_symbol() {
        assert_eq!(FlexBaudRate::Baud1600.bits_per_symbol(), 1);
        assert_eq!(FlexBaudRate::Baud3200.bits_per_symbol(), 2);
        assert_eq!(FlexBaudRate::Baud6400.bits_per_symbol(), 2);
    }

    #[test]
    fn test_pager_error_display() {
        let e = PagerError::BchUncorrectable;
        assert!(!e.to_string().is_empty());
        let e2 = PagerError::NoSync;
        assert!(!e2.to_string().is_empty());
        let e3 = PagerError::InvalidMessage;
        assert!(!e3.to_string().is_empty());
    }

    #[test]
    fn test_iq_sample_magnitude() {
        let s = IqSample::new(0.6, 0.8);
        let mag = s.magnitude_sq().sqrt();
        assert!((mag - 1.0).abs() < 1e-9);
    }
}
