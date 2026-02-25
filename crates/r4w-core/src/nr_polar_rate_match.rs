//! 5G NR Polar Code Rate Matching — 3GPP TS 38.212 Section 5.3.1
//!
//! Implements the complete 5G NR polar coding chain:
//! - Polar encoder (Arikan butterfly F⊗n)
//! - CRC attachment (CRC-24C, CRC-11, CRC-6)
//! - Distributed CRC interleaving for SCL early termination
//! - Channel reliability ordering (Bhattacharyya-ordered Q-sequence)
//! - Frozen bit mask generation
//! - Rate matching (sub-block interleaving, circular buffer: puncturing/shortening/repetition)
//! - Channel interleaving (triangular pattern for PUCCH)
//! - CRC-aided Successive Cancellation List (SCL) decoding, L=1/2/4/8
//! - Code block segmentation
//!
//! ## References
//! - 3GPP TS 38.212 v17.4.0, Section 5.3.1 (Polar coding)
//! - Arikan, E. (2009). Channel polarization.
//!
//! ## Example
//!
//! ```rust
//! use r4w_core::nr_polar_rate_match::{NrPolarEncoder, NrPolarDecoder, CrcType, RateMatchMode};
//!
//! let payload = vec![true; 20];
//! let enc = NrPolarEncoder::new_pdcch(20, 64);
//! let rate_matched = enc.encode_and_rate_match(&payload, 128, RateMatchMode::Repetition);
//! assert_eq!(rate_matched.len(), 128);
//! ```

// ============================================================
// CONSTANTS
// ============================================================

/// Maximum mother code length N_max = 2^10 = 1024.
pub const N_MAX: usize = 1024;
/// Minimum mother code exponent n_min = 5 → N_min = 32.
pub const N_MIN_EXP: u32 = 5;
/// Minimum mother code length N_min = 32.
pub const N_MIN: usize = 32;
/// Sub-block interleaver row count (TS 38.212 Sec 5.3.1.3).
pub const SUBBLOCK_ROWS: usize = 32;
/// Maximum K_max = 1024.
pub const K_MAX: usize = 1024;
/// SCL decoder maximum list size.
pub const SCL_MAX_LIST: usize = 8;

// ============================================================
// RELIABILITY SEQUENCE COMPUTATION
// ============================================================

/// Compute Bhattacharyya parameters for all N channels.
///
/// For channel `ch` in [0,N), the tree path is determined by the bit-reversal of `ch`.
/// At each stage: going to upper branch (f-node) degrades: z → 2z - z².
/// Going to lower branch (g-node) improves: z → z².
/// Seed: z=0.5 (binary erasure channel at design SNR).
fn bhattacharyya_params(n: usize) -> Vec<f64> {
    let log_n = n.trailing_zeros() as usize;
    (0..n).map(|ch| {
        let ch_rev = bit_reverse(ch, log_n);
        let mut z = 0.5f64;
        for stage in 0..log_n {
            let bit = (ch_rev >> (log_n - 1 - stage)) & 1;
            z = if bit == 0 { 2.0*z - z*z } else { z*z };
        }
        z
    }).collect()
}

/// Inverse of SUBBLOCK_PERM: maps old_row → new_row.
/// SUBBLOCK_PERM_INV[old_row] = new_row where SUBBLOCK_PERM[new_row] == old_row.
const SUBBLOCK_PERM_INV: [usize; 32] = [
     0,  1,  2,  4,  3,  5,  6,  7,  // old_rows 0..7
     8, 10, 12, 14, 16, 18, 20, 22,  // old_rows 8..15
     9, 11, 13, 15, 17, 19, 21, 23,  // old_rows 16..23
    24, 25, 26, 28, 27, 29, 30, 31,  // old_rows 24..31
];

/// Compute the sub-block interleaved position of codeword natural index `i` for
/// a mother code of length `n`.
///
/// Per TS 38.212 Sec 5.3.1.3: fill 32-row × cols matrix row-major, permute rows
/// via SUBBLOCK_PERM, then read column-major. Returns position in [0, n).
fn subblock_interleaved_position(i: usize, n: usize) -> usize {
    let cols = (n + SUBBLOCK_ROWS - 1) / SUBBLOCK_ROWS;
    let row = i / cols;
    let col = i % cols;
    let new_row = SUBBLOCK_PERM_INV[row];
    col * SUBBLOCK_ROWS + new_row
}

/// Compute the Bhattacharyya-based reliability ordering for a given N.
///
/// Returns indices sorted from least reliable (highest z, most error-prone) to most reliable.
/// This is the standard BEC-based polar code channel ordering.
///
/// For rate matching (shortening/puncturing), use `build_frozen_mask_shortening` or similar
/// E-aware functions that override this ordering based on the algebraic constraints of G_N.
pub fn reliability_order_n(n: usize) -> Vec<usize> {
    assert!(n.is_power_of_two() && n <= N_MAX && n >= N_MIN);
    let z = bhattacharyya_params(n);
    let mut order: Vec<usize> = (0..n).collect();
    // Sort by descending Bhattacharyya z: least reliable (largest z) first.
    order.sort_by(|&a, &b| z[b].partial_cmp(&z[a]).unwrap_or(std::cmp::Ordering::Equal));
    order
}

/// Compute the 1024-entry Q-sequence (reliability order for N_max=1024).
pub fn compute_q_sequence() -> [u16; 1024] {
    let order = reliability_order_n(N_MAX);
    let mut result = [0u16; N_MAX];
    for (i, &v) in order.iter().enumerate() {
        result[i] = v as u16;
    }
    result
}

// ============================================================
// CRC TYPE
// ============================================================

/// CRC type for 5G NR polar-coded channels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrcType {
    /// CRC-24C: 24 bits — PDCCH, PBCH. Poly: x^24+x^23+x^21+x^20+x^17+x^15+x^13+x^12+x^8+x^4+x^2+x+1.
    Crc24C,
    /// CRC-11: 11 bits — PUCCH A>19. Poly: x^11+x^10+x^9+x^5+1.
    Crc11,
    /// CRC-6: 6 bits — PUCCH A≤19. Poly: x^6+x^5+1.
    Crc6,
}

impl CrcType {
    /// Number of CRC check bits.
    pub fn len(self) -> usize {
        match self { CrcType::Crc24C => 24, CrcType::Crc11 => 11, CrcType::Crc6 => 6 }
    }

    /// Generator polynomial remainder (degree 0 at bit 0).
    fn poly(self) -> u32 {
        match self {
            CrcType::Crc24C => 0x00B2B117,
            CrcType::Crc11  => 0x00000E21,
            CrcType::Crc6   => 0x00000061,
        }
    }

    /// Compute CRC over bits. Returns CRC bits MSB-first.
    pub fn compute(self, bits: &[bool]) -> Vec<bool> {
        let len = self.len();
        let poly = self.poly();
        let mask = (1u32 << len) - 1;
        let mut reg = 0u32;
        for &b in bits {
            let msb = (reg >> (len - 1)) & 1;
            let fb = msb ^ (b as u32);
            reg = ((reg << 1) & mask) ^ (fb * poly);
        }
        for _ in 0..len {
            let msb = (reg >> (len - 1)) & 1;
            reg = ((reg << 1) & mask) ^ (msb * poly);
        }
        (0..len).rev().map(|i| (reg >> i) & 1 != 0).collect()
    }

    /// Verify CRC: last `len(self)` bits of `bits_with_crc` should match CRC of leading bits.
    pub fn verify(self, bits_with_crc: &[bool]) -> bool {
        let crc_len = self.len();
        if bits_with_crc.len() < crc_len { return false; }
        let n = bits_with_crc.len();
        let expected = self.compute(&bits_with_crc[..n - crc_len]);
        expected.iter().zip(&bits_with_crc[n - crc_len..]).all(|(&e, &r)| e == r)
    }
}

// ============================================================
// DISTRIBUTED CRC INTERLEAVING
// ============================================================

/// Interleave P CRC bits uniformly among A payload bits to form K = A+P bits.
///
/// CRC bit positions are evenly spaced: `pos[i] = round((i+1) * K / (P+1))`.
/// This guarantees no duplicates and enables early SCL termination.
pub fn interleave_crc(payload: &[bool], crc: &[bool]) -> Vec<bool> {
    let a = payload.len();
    let p = crc.len();
    let k = a + p;
    // Evenly space p CRC positions in [0, k): pos[i] = round((i+1)*k/(p+1))
    let crc_positions: Vec<usize> = (0..p)
        .map(|i| ((i + 1) * k / (p + 1)).min(k - 1))
        .collect();
    let crc_set: std::collections::HashSet<usize> = crc_positions.iter().copied().collect();
    let mut out = vec![false; k];
    let mut pi = 0usize;
    let mut ci = 0usize;
    for i in 0..k {
        if crc_set.contains(&i) && ci < p {
            out[i] = crc[ci];
            ci += 1;
        } else if pi < a {
            out[i] = payload[pi];
            pi += 1;
        }
    }
    out
}

/// De-interleave CRC bits from K-bit block.
pub fn deinterleave_crc(bits: &[bool], p: usize) -> (Vec<bool>, Vec<bool>) {
    let k = bits.len();
    let crc_positions: Vec<usize> = (0..p)
        .map(|i| ((i + 1) * k / (p + 1)).min(k - 1))
        .collect();
    let crc_set: std::collections::HashSet<usize> = crc_positions.iter().copied().collect();
    let mut payload = Vec::new();
    let mut crc = Vec::new();
    let mut ci = 0usize;
    for i in 0..k {
        if crc_set.contains(&i) && ci < p {
            crc.push(bits[i]);
            ci += 1;
        } else {
            payload.push(bits[i]);
        }
    }
    (payload, crc)
}

// ============================================================
// FROZEN BIT MASK
// ============================================================

/// Build frozen-bit mask for (N, K): `true` = frozen (unreliable).
pub fn build_frozen_mask(n: usize, k: usize) -> Vec<bool> {
    assert!(n.is_power_of_two() && n >= N_MIN && n <= N_MAX);
    assert!(k <= n);
    let order = reliability_order_n(n);
    // K most reliable channels are info bits (last K in order)
    let info_start = n.saturating_sub(k);
    let mut frozen = vec![true; n];
    for &idx in &order[info_start..] {
        frozen[idx] = false;
    }
    frozen
}

/// Return K most reliable channel indices for (N, K), sorted ascending.
pub fn info_bit_positions(n: usize, k: usize) -> Vec<usize> {
    assert!(n.is_power_of_two() && n >= N_MIN && n <= N_MAX);
    assert!(k <= n);
    let order = reliability_order_n(n);
    let info_start = n.saturating_sub(k);
    let mut pos: Vec<usize> = order[info_start..].to_vec();
    pos.sort_unstable();
    pos
}

/// Compute the set of u-domain positions that MUST be frozen for shortening with parameter E.
///
/// For shortening (TS 38.212 Sec 5.3.1.3), the last N−E bits of the sub-block interleaved
/// codeword must be zero. A u position j must be frozen iff setting u[j]=1 (all others 0)
/// causes any interleaved codeword bit at position ≥ E to be 1.
///
/// Returns a bool mask of length N: `true` = must be frozen for shortening to work.
pub fn shortening_must_freeze(n: usize, e: usize) -> Vec<bool> {
    assert!(n.is_power_of_two() && n >= N_MIN && n <= N_MAX);
    assert!(e < n);
    let log_n = n.trailing_zeros() as usize;
    let mut must_freeze = vec![false; n];
    for j in 0..n {
        // Simulate polar transform on unit vector e_j
        let mut u = vec![false; n];
        u[j] = true;
        polar_transform_helper(&mut u, log_n);
        // Apply sub-block interleave and check if any position >= e is 1
        let interleaved = subblock_interleave_helper(&u);
        if interleaved[e..].iter().any(|&b| b) {
            must_freeze[j] = true;
        }
    }
    must_freeze
}

/// Build frozen-bit mask for (N, K, E) with shortening rate matching.
///
/// Positions that algebraically affect the shortened (zeroed) codeword region are
/// forced frozen first. The K most-reliable remaining channels (by Bhattacharyya) become info bits.
///
/// This ensures codeword[E..N] = 0 after sub-block interleaving, satisfying the shortening constraint.
pub fn build_frozen_mask_shortening(n: usize, k: usize, e: usize) -> Vec<bool> {
    assert!(n.is_power_of_two() && n >= N_MIN && n <= N_MAX);
    assert!(k <= n && e < n);
    let must_freeze = shortening_must_freeze(n, e);
    // Sort by descending Bhattacharyya z (most reliable first)
    let z = bhattacharyya_params(n);
    let mut candidates: Vec<usize> = (0..n).filter(|&j| !must_freeze[j]).collect();
    candidates.sort_by(|&a, &b| z[b].partial_cmp(&z[a]).unwrap_or(std::cmp::Ordering::Equal));
    // Take K most reliable as info positions
    let info_set: std::collections::HashSet<usize> = candidates.iter().take(k).cloned().collect();
    (0..n).map(|i| !info_set.contains(&i)).collect()
}

/// Return K most reliable channel indices for shortening (N, K, E), sorted ascending.
pub fn info_bit_positions_shortening(n: usize, k: usize, e: usize) -> Vec<usize> {
    let frozen = build_frozen_mask_shortening(n, k, e);
    let mut pos: Vec<usize> = (0..n).filter(|&i| !frozen[i]).collect();
    pos.sort_unstable();
    pos
}

/// Helper: polar transform including bit-reversal permutation (reuses bit_reverse).
/// Used internally to compute the G_N matrix structure for rate-matching-aware frozen mask.
fn polar_transform_helper(u: &mut Vec<bool>, log_n: usize) {
    let n = u.len();
    // bit-reversal permutation
    for i in 0..n {
        let j = bit_reverse(i, log_n);
        if i < j { u.swap(i, j); }
    }
    // butterfly stages
    let mut step = 1usize;
    while step < n {
        let mut i = 0;
        while i < n {
            for jj in 0..step {
                let a = u[i + jj];
                let b = u[i + jj + step];
                u[i + jj] = a ^ b;
                u[i + jj + step] = b;
            }
            i += 2 * step;
        }
        step *= 2;
    }
}

/// Helper: sub-block interleave on bool slice.
/// Used to determine which natural codeword positions map to the shortened region.
fn subblock_interleave_helper(codeword: &[bool]) -> Vec<bool> {
    let n = codeword.len();
    let cols = (n + SUBBLOCK_ROWS - 1) / SUBBLOCK_ROWS;
    let mut matrix = vec![vec![false; cols]; SUBBLOCK_ROWS];
    for (idx, &bit) in codeword.iter().enumerate() {
        let row = idx / cols;
        let col = idx % cols;
        if row < SUBBLOCK_ROWS { matrix[row][col] = bit; }
    }
    let mut perm = vec![vec![false; cols]; SUBBLOCK_ROWS];
    for (new_row, &old_row) in SUBBLOCK_PERM.iter().enumerate() {
        perm[new_row] = matrix[old_row].clone();
    }
    let mut out = Vec::with_capacity(n);
    for col in 0..cols {
        for row in 0..SUBBLOCK_ROWS { out.push(perm[row][col]); }
    }
    out.truncate(n);
    out
}

// ============================================================
// POLAR TRANSFORM
// ============================================================

/// Arikan's polar butterfly transform in-place: x = u × G_N.
///
/// G_N = B_N × F^⊗n where B_N is the bit-reversal permutation.
pub fn polar_transform(u: &mut Vec<bool>) {
    let n = u.len();
    debug_assert!(n.is_power_of_two());
    bit_reversal_permute(u);
    let mut step = 1usize;
    while step < n {
        let mut i = 0;
        while i < n {
            for j in 0..step {
                let a = u[i + j];
                let b = u[i + j + step];
                u[i + j] = a ^ b;
                u[i + j + step] = b;
            }
            i += 2 * step;
        }
        step *= 2;
    }
}

/// In-place bit-reversal permutation.
pub fn bit_reversal_permute(v: &mut Vec<bool>) {
    let n = v.len();
    if n <= 1 { return; }
    let log_n = n.trailing_zeros() as usize;
    for i in 0..n {
        let j = bit_reverse(i, log_n);
        if i < j { v.swap(i, j); }
    }
}

/// Bit-reverse an integer of width `bits`.
fn bit_reverse(mut x: usize, bits: usize) -> usize {
    let mut r = 0usize;
    for _ in 0..bits { r = (r << 1) | (x & 1); x >>= 1; }
    r
}

// ============================================================
// RATE MATCHING
// ============================================================

/// Rate matching mode per TS 38.212 Sec 5.3.1.3.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateMatchMode {
    /// Puncturing: skip first N-E bits. E < N.
    Puncturing,
    /// Shortening: output first E bits (last N-E are known 0). E < N.
    Shortening,
    /// Repetition: read E bits cyclically from N-bit buffer. E >= N.
    Repetition,
}

impl RateMatchMode {
    /// Auto-select based on E vs N.
    pub fn auto(e: usize, n: usize) -> Self {
        if e >= n { RateMatchMode::Repetition }
        else if e * 4 < n * 3 { RateMatchMode::Puncturing }
        else { RateMatchMode::Shortening }
    }
}

/// Sub-block interleaver row permutation — TS 38.212 Table 5.3.1.3-1.
const SUBBLOCK_PERM: [usize; 32] = [
    0, 1, 2, 4, 3, 5, 6, 7, 8, 16, 9, 17, 10, 18, 11, 19,
    12, 20, 13, 21, 14, 22, 15, 23, 24, 25, 26, 28, 27, 29, 30, 31,
];

/// Sub-block interleaving: N-bit codeword → 32-row matrix → permute rows → column-major read.
pub fn subblock_interleave(codeword: &[bool]) -> Vec<bool> {
    let n = codeword.len();
    let cols = (n + SUBBLOCK_ROWS - 1) / SUBBLOCK_ROWS;
    let mut matrix = vec![vec![false; cols]; SUBBLOCK_ROWS];
    for (idx, &bit) in codeword.iter().enumerate() {
        let row = idx / cols;
        let col = idx % cols;
        if row < SUBBLOCK_ROWS { matrix[row][col] = bit; }
    }
    // Apply row permutation
    let mut perm = vec![vec![false; cols]; SUBBLOCK_ROWS];
    for (new_row, &old_row) in SUBBLOCK_PERM.iter().enumerate() {
        perm[new_row] = matrix[old_row].clone();
    }
    // Read column-major
    let mut out = Vec::with_capacity(n);
    for col in 0..cols {
        for row in 0..SUBBLOCK_ROWS { out.push(perm[row][col]); }
    }
    out.truncate(n);
    out
}

/// Inverse sub-block interleaving.
pub fn subblock_deinterleave(interleaved: &[bool], n: usize) -> Vec<bool> {
    let cols = (n + SUBBLOCK_ROWS - 1) / SUBBLOCK_ROWS;
    let mut perm = vec![vec![false; cols]; SUBBLOCK_ROWS];
    for (idx, &bit) in interleaved.iter().take(n).enumerate() {
        let col = idx / SUBBLOCK_ROWS;
        let row = idx % SUBBLOCK_ROWS;
        if col < cols { perm[row][col] = bit; }
    }
    // Inverse permutation
    let mut matrix = vec![vec![false; cols]; SUBBLOCK_ROWS];
    for (new_row, &old_row) in SUBBLOCK_PERM.iter().enumerate() {
        matrix[old_row] = perm[new_row].clone();
    }
    // Read row-major
    let mut out = Vec::with_capacity(n);
    for row in 0..SUBBLOCK_ROWS {
        for col in 0..cols { out.push(matrix[row][col]); }
    }
    out.truncate(n);
    out
}

/// Apply rate matching: sub-block interleave → circular buffer → select E bits.
pub fn rate_match(codeword: &[bool], e: usize, mode: RateMatchMode) -> Vec<bool> {
    let n = codeword.len();
    let y = subblock_interleave(codeword);
    let mut out = Vec::with_capacity(e);
    match mode {
        RateMatchMode::Repetition => {
            for i in 0..e { out.push(y[i % n]); }
        }
        RateMatchMode::Puncturing => {
            let start = n.saturating_sub(e);
            out.extend_from_slice(&y[start..n]);
        }
        RateMatchMode::Shortening => {
            let take = e.min(n);
            out.extend_from_slice(&y[..take]);
        }
    }
    out
}

/// De-rate-match: E received LLRs → N LLRs for polar decoder.
pub fn derate_match(llrs: &[f64], n: usize, mode: RateMatchMode) -> Vec<f64> {
    let e = llrs.len();
    let mut y = vec![0.0f64; n];
    match mode {
        RateMatchMode::Repetition => {
            for (i, &llr) in llrs.iter().enumerate() { y[i % n] += llr; }
        }
        RateMatchMode::Puncturing => {
            let start = n.saturating_sub(e);
            y[start..n].copy_from_slice(&llrs[..e.min(n)]);
        }
        RateMatchMode::Shortening => {
            let take = e.min(n);
            y[..take].copy_from_slice(&llrs[..take]);
            for v in y[take..].iter_mut() { *v = 1e10; } // known 0
        }
    }
    subblock_deinterleave_f64(&y, n)
}

fn subblock_deinterleave_f64(interleaved: &[f64], n: usize) -> Vec<f64> {
    let cols = (n + SUBBLOCK_ROWS - 1) / SUBBLOCK_ROWS;
    let mut perm = vec![vec![0.0f64; cols]; SUBBLOCK_ROWS];
    for (idx, &val) in interleaved.iter().take(n).enumerate() {
        let col = idx / SUBBLOCK_ROWS;
        let row = idx % SUBBLOCK_ROWS;
        if col < cols { perm[row][col] = val; }
    }
    let mut matrix = vec![vec![0.0f64; cols]; SUBBLOCK_ROWS];
    for (new_row, &old_row) in SUBBLOCK_PERM.iter().enumerate() {
        matrix[old_row] = perm[new_row].clone();
    }
    let mut out = Vec::with_capacity(n);
    for row in 0..SUBBLOCK_ROWS {
        for col in 0..cols { out.push(matrix[row][col]); }
    }
    out.truncate(n);
    out
}

// ============================================================
// CHANNEL INTERLEAVER
// ============================================================

/// Channel interleaver for PUCCH per TS 38.212 Section 5.3.1.6.
///
/// Triangular write pattern: write diagonally, read column-by-column.
pub fn channel_interleave(bits: &[bool]) -> Vec<bool> {
    let e = bits.len();
    let t = triangular_t(e);
    let mut matrix: Vec<Vec<Option<bool>>> = vec![vec![None; t]; t];
    let mut idx = 0;
    'fill: for row in 0..t {
        for col in 0..=row {
            if idx >= e { break 'fill; }
            matrix[row][col] = Some(bits[idx]);
            idx += 1;
        }
    }
    let mut out = Vec::with_capacity(e);
    'read: for col in 0..t {
        for row in col..t {
            if let Some(b) = matrix[row][col] {
                out.push(b);
                if out.len() >= e { break 'read; }
            }
        }
    }
    out
}

/// Inverse channel interleaver.
pub fn channel_deinterleave(bits: &[bool]) -> Vec<bool> {
    let e = bits.len();
    let t = triangular_t(e);
    let mut matrix: Vec<Vec<Option<bool>>> = vec![vec![None; t]; t];
    let mut idx = 0;
    'fill: for col in 0..t {
        for row in col..t {
            if idx >= e { break 'fill; }
            matrix[row][col] = Some(bits[idx]);
            idx += 1;
        }
    }
    let mut out = Vec::with_capacity(e);
    'read: for row in 0..t {
        for col in 0..=row {
            if let Some(b) = matrix[row][col] {
                out.push(b);
                if out.len() >= e { break 'read; }
            }
        }
    }
    out
}

fn triangular_t(e: usize) -> usize {
    if e == 0 { return 1; }
    let mut t = 1;
    while t * (t + 1) / 2 < e { t += 1; }
    t
}

// ============================================================
// PARAMETER COMPUTATION
// ============================================================

/// NR polar code parameters.
#[derive(Debug, Clone, Copy)]
pub struct PolarParams {
    pub a: usize,
    pub k: usize,
    pub n: usize,
    pub e: usize,
    pub mode: RateMatchMode,
}

impl PolarParams {
    pub fn for_pdcch(a: usize, e: usize) -> Self {
        let k = a + CrcType::Crc24C.len();
        let n = compute_n(k, e, 9);
        PolarParams { a, k, n, e, mode: RateMatchMode::auto(e, n) }
    }
    pub fn for_pbch() -> Self {
        let a = 32; let e = 864;
        let k = a + CrcType::Crc24C.len();
        let n = 512;
        PolarParams { a, k, n, e, mode: RateMatchMode::auto(e, n) }
    }
    pub fn for_pucch(a: usize, e: usize) -> Self {
        let crc_len = if a > 19 { CrcType::Crc11.len() } else { CrcType::Crc6.len() };
        let k = a + crc_len;
        let n = compute_n(k, e, 10);
        PolarParams { a, k, n, e, mode: RateMatchMode::auto(e, n) }
    }
}

/// Compute N: smallest power-of-2 satisfying K ≤ N and rate constraint.
pub fn compute_n(k: usize, e: usize, n_max_exp: u32) -> usize {
    let n_max = 1usize << n_max_exp;
    // n1: smallest power-of-2 ≥ K
    let mut n1 = N_MIN;
    while n1 < k { n1 *= 2; }
    // n2: from lower rate bound E/N ≥ 1/8 → N ≤ 8E
    let mut n2 = N_MIN;
    while n2 * 2 <= 8 * e && n2 < n_max { n2 *= 2; }
    n1.max(N_MIN).min(n2).min(n_max).next_power_of_two().min(n_max)
}

// ============================================================
// NR POLAR ENCODER
// ============================================================

/// 5G NR Polar Encoder.
#[derive(Debug, Clone)]
pub struct NrPolarEncoder {
    pub n: usize,
    pub k: usize,
    pub a: usize,
    pub crc: CrcType,
    frozen: Vec<bool>,
    info_pos: Vec<usize>,
    pub distributed_crc: bool,
}

impl NrPolarEncoder {
    pub fn new_pdcch(a: usize, n: usize) -> Self {
        Self::new(a, n, CrcType::Crc24C, false)
    }
    pub fn new_pbch(a: usize, n: usize) -> Self {
        Self::new(a, n, CrcType::Crc24C, true)
    }
    pub fn new_pucch(a: usize, n: usize) -> Self {
        let crc = if a > 19 { CrcType::Crc11 } else { CrcType::Crc6 };
        Self::new(a, n, crc, false)
    }
    /// Construct PUCCH encoder with shortening-aware frozen mask.
    ///
    /// When E < N (shortening), the standard reliability ordering cannot guarantee
    /// that the shortened positions are frozen. This constructor uses `build_frozen_mask_shortening`
    /// to compute the algebraically correct frozen mask for the given (A, N, E).
    pub fn new_pucch_shortened(a: usize, n: usize, e: usize) -> Self {
        let crc = if a > 19 { CrcType::Crc11 } else { CrcType::Crc6 };
        let k = a + crc.len();
        let frozen = build_frozen_mask_shortening(n, k, e);
        let info_pos = {
            let mut pos: Vec<usize> = (0..n).filter(|&i| !frozen[i]).collect();
            pos.sort_unstable();
            pos
        };
        NrPolarEncoder { n, k, a, crc, frozen, info_pos, distributed_crc: false }
    }
    pub fn new(a: usize, n: usize, crc: CrcType, distributed_crc: bool) -> Self {
        let k = a + crc.len();
        let frozen = build_frozen_mask(n, k);
        let info_pos = info_bit_positions(n, k);
        NrPolarEncoder { n, k, a, crc, frozen, info_pos, distributed_crc }
    }

    /// Encode: CRC attachment → bit insertion → polar transform → N-bit codeword.
    pub fn encode(&self, payload: &[bool]) -> Vec<bool> {
        assert_eq!(payload.len(), self.a);
        let crc_bits = self.crc.compute(payload);
        let k_bits: Vec<bool> = if self.distributed_crc {
            interleave_crc(payload, &crc_bits)
        } else {
            let mut b = payload.to_vec();
            b.extend_from_slice(&crc_bits);
            b
        };
        let mut u = vec![false; self.n];
        for (i, &pos) in self.info_pos.iter().enumerate() {
            u[pos] = k_bits[i];
        }
        polar_transform(&mut u);
        u
    }

    pub fn encode_and_rate_match(&self, payload: &[bool], e: usize, mode: RateMatchMode) -> Vec<bool> {
        rate_match(&self.encode(payload), e, mode)
    }
}

// ============================================================
// SC DECODER (L=1)
// ============================================================

/// Successive Cancellation decoder (list size 1).
#[derive(Debug, Clone)]
pub struct ScDecoder {
    n: usize,
    frozen: Vec<bool>,
    info_pos: Vec<usize>,
}

impl ScDecoder {
    pub fn new(n: usize, k: usize) -> Self {
        ScDecoder { n, frozen: build_frozen_mask(n, k), info_pos: info_bit_positions(n, k) }
    }
    pub fn decode(&self, llrs: &[f64]) -> Vec<bool> {
        assert_eq!(llrs.len(), self.n);
        let all = sc_decode_5gnr(llrs, &self.frozen);
        self.info_pos.iter().map(|&p| all[p]).collect()
    }
}

/// SC decode for the 5G NR polar encoder (G_N = B_N × F^⊗n).
///
/// The 5G NR encoder applies bit-reversal first, then bottom-up butterfly steps.
/// The SC decoder must account for this by:
/// 1. Converting the frozen mask from u-domain to BR(u)-domain.
/// 2. Running the recursive SC decode in BR(u)-domain.
/// 3. Converting the result back to u-domain.
///
/// Inside the recursion, each recursive call peels off the last encoder stage
/// (step = n/2). The partial sums for the g-function are computed by re-encoding
/// the upper-half decisions using the bottom-up (BU) sub-encoder.
fn sc_decode_5gnr(llrs: &[f64], frozen_u: &[bool]) -> Vec<bool> {
    let n = llrs.len();
    let log_n = n.trailing_zeros() as usize;
    // Convert frozen mask from u-domain to BR(u)-domain
    let frozen_br: Vec<bool> = (0..n).map(|i| frozen_u[bit_reverse(i, log_n)]).collect();
    // Decode in BR(u)-domain; returns BR(u_hat)
    let br_u_hat = sc_decode_br(llrs, &frozen_br);
    // Convert back: u_hat[j] = br_u_hat[BR(j)]
    (0..n).map(|j| br_u_hat[bit_reverse(j, log_n)]).collect()
}

/// Recursive SC decode in the BR(u)-domain.
///
/// The 5G NR encoder for N bits, after removing the outermost butterfly (step=N/2),
/// decomposes into two independent N/2-sized BU sub-encoders applied to the
/// first and second halves of BR_N(u). The output of this function is BR(u_hat).
///
/// The g-function partial sums use `bu_encode` (bottom-up with no bit-reversal)
/// because the sub-encoder for each half IS the BU encoder applied to BR_N(u)[0..N/2]
/// (the BR step was already embedded in the top-level permutation).
fn sc_decode_br(llrs: &[f64], frozen_br: &[bool]) -> Vec<bool> {
    let n = llrs.len();
    if n == 1 {
        return vec![if frozen_br[0] { false } else { llrs[0] < 0.0 }];
    }
    let half = n / 2;
    let lu: Vec<f64> = (0..half).map(|i| f_fn(llrs[i], llrs[i + half])).collect();
    let uh = sc_decode_br(&lu, &frozen_br[..half]);
    // Re-encode uh with BU encoder to get the partial sums needed by the g-function
    let uh_ps = bu_encode_partial(&uh);
    let ll: Vec<f64> = (0..half).map(|i| g_fn(llrs[i], llrs[i + half], uh_ps[i])).collect();
    let lh = sc_decode_br(&ll, &frozen_br[half..]);
    let mut out = Vec::with_capacity(n);
    out.extend_from_slice(&uh);
    out.extend_from_slice(&lh);
    out
}

/// Bottom-up polar encoding without bit-reversal (used for partial sum computation).
fn bu_encode_partial(u: &[bool]) -> Vec<bool> {
    let n = u.len();
    let mut v = u.to_vec();
    let mut step = 1usize;
    while step < n {
        let mut i = 0;
        while i < n {
            for j in 0..step {
                let a = v[i + j];
                let b = v[i + j + step];
                v[i + j] = a ^ b;
                v[i + j + step] = b;
            }
            i += 2 * step;
        }
        step *= 2;
    }
    v
}

/// F-function (min-sum): sign(a)·sign(b)·min(|a|,|b|).
#[inline]
fn f_fn(a: f64, b: f64) -> f64 {
    let s = if (a < 0.0) ^ (b < 0.0) { -1.0 } else { 1.0 };
    s * a.abs().min(b.abs())
}

/// G-function: b + (1-2u)·a.
#[inline]
fn g_fn(a: f64, b: f64, u: bool) -> f64 { if u { b - a } else { b + a } }

// ============================================================
// SCL DECODER (L = 1/2/4/8)
// ============================================================

#[derive(Clone)]
struct SclPath { bits: Vec<bool>, metric: f64 }

/// CRC-Aided Successive Cancellation List decoder.
#[derive(Debug, Clone)]
pub struct SclDecoder {
    n: usize,
    list_size: usize,
    frozen: Vec<bool>,
    info_pos: Vec<usize>,
    crc: Option<CrcType>,
}

impl SclDecoder {
    pub fn new(n: usize, k: usize, list_size: usize, crc: Option<CrcType>) -> Self {
        assert!(list_size <= SCL_MAX_LIST);
        SclDecoder {
            n, list_size,
            frozen: build_frozen_mask(n, k),
            info_pos: info_bit_positions(n, k),
            crc,
        }
    }

    /// Construct SclDecoder with a pre-computed frozen mask (for rate-matching-aware construction).
    pub fn new_with_mask(n: usize, list_size: usize, frozen: Vec<bool>, crc: Option<CrcType>) -> Self {
        assert!(list_size <= SCL_MAX_LIST);
        assert_eq!(frozen.len(), n);
        let info_pos: Vec<usize> = (0..n).filter(|&i| !frozen[i]).collect();
        SclDecoder { n, list_size, frozen, info_pos, crc }
    }

    pub fn decode(&self, llrs: &[f64]) -> Vec<bool> {
        assert_eq!(llrs.len(), self.n);
        let best = self.decode_all_bits(llrs);
        self.info_pos.iter().map(|&p| best[p]).collect()
    }

    pub fn decode_all_bits(&self, llrs: &[f64]) -> Vec<bool> {
        let log_n = self.n.trailing_zeros() as usize;
        // Convert frozen mask to BR-domain: frozen_br[i] = frozen_u[BR(i)]
        let frozen_br: Vec<bool> = (0..self.n).map(|i| self.frozen[bit_reverse(i, log_n)]).collect();
        // SCL operates in BR(u)-domain. Each path stores BR(u_hat) indexed by BR-domain position.
        let mut paths = vec![SclPath { bits: vec![false; self.n], metric: 0.0 }];
        for br_phase in 0..self.n {
            let is_frozen = frozen_br[br_phase];
            let mut new_paths: Vec<SclPath> = Vec::new();
            for path in &paths {
                // path.bits is in BR-domain; leaf_llr_br expects BR-domain decided
                let leaf = leaf_llr_br(llrs, &path.bits, br_phase, self.n);
                if is_frozen {
                    let mut p = path.clone();
                    p.bits[br_phase] = false;
                    if leaf < 0.0 { p.metric += leaf.abs(); }
                    new_paths.push(p);
                } else {
                    let mut p0 = path.clone();
                    p0.bits[br_phase] = false;
                    if leaf < 0.0 { p0.metric += leaf.abs(); }
                    new_paths.push(p0);
                    let mut p1 = path.clone();
                    p1.bits[br_phase] = true;
                    if leaf >= 0.0 { p1.metric += leaf; }
                    new_paths.push(p1);
                }
            }
            new_paths.sort_by(|a, b| a.metric.partial_cmp(&b.metric).unwrap());
            new_paths.truncate(self.list_size);
            paths = new_paths;
        }
        // Convert best path from BR-domain back to u-domain: u_hat[j] = br_hat[BR(j)]
        let br_to_u = |br_bits: &Vec<bool>| -> Vec<bool> {
            (0..self.n).map(|j| br_bits[bit_reverse(j, log_n)]).collect()
        };
        if let Some(crc_type) = self.crc {
            for path in &paths {
                let u_bits = br_to_u(&path.bits);
                let k_bits: Vec<bool> = self.info_pos.iter().map(|&p| u_bits[p]).collect();
                if crc_type.verify(&k_bits) { return u_bits; }
            }
        }
        br_to_u(&paths[0].bits)
    }
}

/// Compute LLR at BR-domain leaf `br_phase` from root LLRs and BR-domain bit decisions.
///
/// `decided` holds the bits decided so far indexed by BR-domain position.
/// The g-function partial sums use `bu_encode_partial` to convert the upper-half
/// BR-domain bits to the intermediate w-domain needed by the g-function.
fn leaf_llr_br(llrs: &[f64], decided: &[bool], br_phase: usize, n: usize) -> f64 {
    if n == 1 { return llrs[0]; }
    let half = n / 2;
    if br_phase < half {
        let left: Vec<f64> = (0..half).map(|i| f_fn(llrs[i], llrs[i + half])).collect();
        leaf_llr_br(&left, decided, br_phase, half)
    } else {
        // Compute partial sums by BU-encoding the upper-half BR-domain decisions
        let uh_ps = bu_encode_partial(&decided[..half]);
        let right: Vec<f64> = (0..half).map(|i| g_fn(llrs[i], llrs[i + half], uh_ps[i])).collect();
        leaf_llr_br(&right, &decided[half..], br_phase - half, half)
    }
}

// ============================================================
// NR POLAR DECODER (FULL CHAIN)
// ============================================================

/// 5G NR Polar Decoder: derate-match → SCL decode → CRC verify → payload.
#[derive(Debug, Clone)]
pub struct NrPolarDecoder {
    pub n: usize,
    pub k: usize,
    pub a: usize,
    pub crc: CrcType,
    scl: SclDecoder,
    pub distributed_crc: bool,
}

impl NrPolarDecoder {
    pub fn new_pdcch(a: usize, n: usize, list_size: usize) -> Self {
        let crc = CrcType::Crc24C;
        let k = a + crc.len();
        NrPolarDecoder { n, k, a, crc, scl: SclDecoder::new(n, k, list_size, Some(crc)), distributed_crc: false }
    }
    pub fn new_pbch(a: usize, n: usize, list_size: usize) -> Self {
        let crc = CrcType::Crc24C;
        let k = a + crc.len();
        NrPolarDecoder { n, k, a, crc, scl: SclDecoder::new(n, k, list_size, Some(crc)), distributed_crc: true }
    }
    pub fn new_pucch(a: usize, n: usize, list_size: usize) -> Self {
        let crc = if a > 19 { CrcType::Crc11 } else { CrcType::Crc6 };
        let k = a + crc.len();
        NrPolarDecoder { n, k, a, crc, scl: SclDecoder::new(n, k, list_size, Some(crc)), distributed_crc: false }
    }
    /// Construct PUCCH decoder with shortening-aware frozen mask.
    ///
    /// Matches `NrPolarEncoder::new_pucch_shortened` — must use the same (a, n, e) values.
    pub fn new_pucch_shortened(a: usize, n: usize, e: usize, list_size: usize) -> Self {
        let crc = if a > 19 { CrcType::Crc11 } else { CrcType::Crc6 };
        let k = a + crc.len();
        let frozen = build_frozen_mask_shortening(n, k, e);
        NrPolarDecoder {
            n, k, a, crc,
            scl: SclDecoder::new_with_mask(n, list_size, frozen, Some(crc)),
            distributed_crc: false,
        }
    }

    /// Decode rate-matched LLRs → payload bits (None if CRC fails).
    pub fn decode_rate_matched(&self, llrs: &[f64], mode: RateMatchMode) -> Option<Vec<bool>> {
        let n_llrs = derate_match(llrs, self.n, mode);
        let k_bits = self.scl.decode(&n_llrs);
        let (payload, crc_ok) = if self.distributed_crc {
            let (p, c) = deinterleave_crc(&k_bits, self.crc.len());
            let expected = self.crc.compute(&p);
            (p, expected == c)
        } else {
            let ok = self.crc.verify(&k_bits);
            let p = k_bits[..self.a.min(k_bits.len())].to_vec();
            (p, ok)
        };
        if crc_ok { Some(payload) } else { None }
    }
}

// ============================================================
// CODE BLOCK SEGMENTATION
// ============================================================

/// A code block segment with its per-segment CRC.
#[derive(Debug, Clone)]
pub struct CodeBlockSegment {
    pub segment_id: usize,
    pub total_segments: usize,
    pub payload: Vec<bool>,
    pub crc: Vec<bool>,
}

/// Segment payload into code blocks (K_max = 1024 per block).
pub fn segment_payload(payload: &[bool], crc_type: CrcType) -> Vec<CodeBlockSegment> {
    let a = payload.len();
    let a_max = K_MAX - crc_type.len();
    if a <= a_max {
        let crc = crc_type.compute(payload);
        return vec![CodeBlockSegment { segment_id: 0, total_segments: 1, payload: payload.to_vec(), crc }];
    }
    let c = (a + a_max - 1) / a_max;
    let seg_size = (a + c - 1) / c;
    let mut start = 0;
    (0..c).map(|i| {
        let end = (start + seg_size).min(a);
        let seg = payload[start..end].to_vec();
        let crc = crc_type.compute(&seg);
        start = end;
        CodeBlockSegment { segment_id: i, total_segments: c, payload: seg, crc }
    }).collect()
}

/// Reassemble segments, verifying per-segment CRC. Returns None on CRC failure.
pub fn reassemble_segments(segments: &[CodeBlockSegment], crc_type: CrcType) -> Option<Vec<bool>> {
    let mut out = Vec::new();
    for seg in segments {
        if crc_type.compute(&seg.payload) != seg.crc { return None; }
        out.extend_from_slice(&seg.payload);
    }
    Some(out)
}

// ============================================================
// UTILITIES
// ============================================================

/// Bytes → MSB-first bit vector.
pub fn bytes_to_bits(bytes: &[u8]) -> Vec<bool> {
    let mut bits = Vec::with_capacity(bytes.len() * 8);
    for &b in bytes {
        for i in (0..8).rev() { bits.push((b >> i) & 1 != 0); }
    }
    bits
}

/// MSB-first bit vector → bytes (zero-padded).
pub fn bits_to_bytes(bits: &[bool]) -> Vec<u8> {
    let n = (bits.len() + 7) / 8;
    let mut bytes = vec![0u8; n];
    for (i, &b) in bits.iter().enumerate() {
        if b { bytes[i / 8] |= 1 << (7 - (i % 8)); }
    }
    bytes
}

/// Bits → LLRs: false → +snr, true → -snr (BPSK soft values).
pub fn bits_to_llrs(bits: &[bool], snr: f64) -> Vec<f64> {
    bits.iter().map(|&b| if b { -snr } else { snr }).collect()
}

// ============================================================
// TESTS
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    // --------------------------------------------------------
    // CRC Tests
    // --------------------------------------------------------

    #[test]
    fn test_crc6_roundtrip() {
        let bits: Vec<bool> = (0..12).map(|i| i % 2 == 0).collect();
        let crc = CrcType::Crc6.compute(&bits);
        assert_eq!(crc.len(), 6);
        let mut w = bits.clone(); w.extend_from_slice(&crc);
        assert!(CrcType::Crc6.verify(&w));
    }

    #[test]
    fn test_crc11_roundtrip() {
        let bits = bytes_to_bits(&[0xAB, 0xCD]);
        let crc = CrcType::Crc11.compute(&bits);
        assert_eq!(crc.len(), 11);
        let mut w = bits.clone(); w.extend_from_slice(&crc);
        assert!(CrcType::Crc11.verify(&w));
    }

    #[test]
    fn test_crc24c_roundtrip() {
        let bits = bytes_to_bits(&[0xDE, 0xAD, 0xBE, 0xEF]);
        let crc = CrcType::Crc24C.compute(&bits);
        assert_eq!(crc.len(), 24);
        let mut w = bits.clone(); w.extend_from_slice(&crc);
        assert!(CrcType::Crc24C.verify(&w));
    }

    #[test]
    fn test_crc24c_detects_single_bit_error() {
        let bits = bytes_to_bits(&[0x12, 0x34, 0x56]);
        let crc = CrcType::Crc24C.compute(&bits);
        let mut w = bits.clone(); w.extend_from_slice(&crc);
        w[7] = !w[7];
        assert!(!CrcType::Crc24C.verify(&w));
    }

    #[test]
    fn test_crc11_detects_error() {
        let bits = vec![false; 20];
        let crc = CrcType::Crc11.compute(&bits);
        let mut w = bits.clone(); w.extend_from_slice(&crc);
        w[0] = true;
        assert!(!CrcType::Crc11.verify(&w));
    }

    #[test]
    fn test_crc6_zero_payload() {
        let bits = vec![false; 10];
        let crc = CrcType::Crc6.compute(&bits);
        let mut w = bits.clone(); w.extend_from_slice(&crc);
        assert!(CrcType::Crc6.verify(&w));
    }

    #[test]
    fn test_crc_different_payloads_differ() {
        let a = bytes_to_bits(&[0xAA]);
        let b = bytes_to_bits(&[0x55]);
        assert_ne!(CrcType::Crc24C.compute(&a), CrcType::Crc24C.compute(&b));
    }

    // --------------------------------------------------------
    // Distributed CRC Tests
    // --------------------------------------------------------

    #[test]
    fn test_distributed_crc_roundtrip_small() {
        let payload: Vec<bool> = (0..6).map(|i| i % 2 == 0).collect();
        let crc = CrcType::Crc6.compute(&payload);
        let iw = interleave_crc(&payload, &crc);
        assert_eq!(iw.len(), 12);
        let (rp, rc) = deinterleave_crc(&iw, crc.len());
        assert_eq!(rp, payload);
        assert_eq!(rc, crc);
    }

    #[test]
    fn test_distributed_crc_roundtrip_crc24c() {
        let payload: Vec<bool> = bytes_to_bits(&[0xCA, 0xFE]);
        let crc = CrcType::Crc24C.compute(&payload);
        let iw = interleave_crc(&payload, &crc);
        assert_eq!(iw.len(), payload.len() + 24);
        let (rp, rc) = deinterleave_crc(&iw, 24);
        assert_eq!(rp, payload);
        assert_eq!(rc, crc);
    }

    #[test]
    fn test_distributed_crc_no_duplicate_positions() {
        // Verify CRC positions are unique for various (A, P) combos
        for &(a, p) in &[(10, 6), (16, 11), (41, 24), (100, 24)] {
            let k = a + p;
            let positions: Vec<usize> = (0..p).map(|i| ((i+1)*k/(p+1)).min(k-1)).collect();
            let unique: std::collections::HashSet<usize> = positions.iter().copied().collect();
            assert_eq!(unique.len(), p, "Duplicate CRC positions for A={} P={}", a, p);
        }
    }

    #[test]
    fn test_distributed_crc_output_length() {
        let payload = vec![false; 41];
        let crc = CrcType::Crc24C.compute(&payload);
        let out = interleave_crc(&payload, &crc);
        assert_eq!(out.len(), 65); // 41 + 24
    }

    // --------------------------------------------------------
    // Bhattacharyya and Frozen Mask Tests
    // --------------------------------------------------------

    #[test]
    fn test_bhattacharyya_n8_ordering() {
        // ch[7] should be most reliable (z closest to 0)
        let z = bhattacharyya_params(8);
        assert!(z[7] < z[0], "ch[7] (all-lower) should be more reliable than ch[0] (all-upper)");
        assert!(z[7] < 0.01, "ch[7] z should be near 0");
        assert!(z[0] > 0.9, "ch[0] z should be near 1");
    }

    #[test]
    fn test_reliability_order_is_permutation_n32() {
        let order = reliability_order_n(32);
        assert_eq!(order.len(), 32);
        let mut seen = [false; 32];
        for &v in &order { seen[v] = true; }
        assert!(seen.iter().all(|&s| s));
    }

    #[test]
    fn test_frozen_mask_k_info_bits() {
        for &(n, k) in &[(32, 10), (64, 24), (128, 64)] {
            let frozen = build_frozen_mask(n, k);
            let count = frozen.iter().filter(|&&f| !f).count();
            assert_eq!(count, k);
        }
    }

    #[test]
    fn test_frozen_mask_all_frozen() {
        assert!(build_frozen_mask(32, 0).iter().all(|&f| f));
    }

    #[test]
    fn test_frozen_mask_all_info() {
        assert!(build_frozen_mask(64, 64).iter().all(|&f| !f));
    }

    #[test]
    fn test_info_positions_sorted_and_in_range() {
        let n = 128; let k = 48;
        let pos = info_bit_positions(n, k);
        assert_eq!(pos.len(), k);
        for i in 1..pos.len() { assert!(pos[i] > pos[i-1]); }
        for &p in &pos { assert!(p < n); }
    }

    // --------------------------------------------------------
    // Polar Transform Tests
    // --------------------------------------------------------

    #[test]
    fn test_polar_transform_n2() {
        let mut u = vec![true, true]; // [1,1]
        polar_transform(&mut u);
        assert_eq!(u, vec![false, true]); // [1^1, 1]
    }

    #[test]
    fn test_polar_transform_involution_n8() {
        let orig: Vec<bool> = (0..8).map(|i| i % 3 < 2).collect();
        let mut u = orig.clone();
        polar_transform(&mut u); polar_transform(&mut u);
        assert_eq!(u, orig);
    }

    #[test]
    fn test_polar_transform_involution_n64() {
        let orig: Vec<bool> = (0..64).map(|i| i % 5 < 2).collect();
        let mut u = orig.clone();
        polar_transform(&mut u); polar_transform(&mut u);
        assert_eq!(u, orig);
    }

    #[test]
    fn test_polar_transform_all_zeros() {
        let mut u = vec![false; 32];
        polar_transform(&mut u);
        assert_eq!(u, vec![false; 32]);
    }

    #[test]
    fn test_bit_reverse_values() {
        assert_eq!(bit_reverse(0b001, 3), 0b100);
        assert_eq!(bit_reverse(0b110, 3), 0b011);
        assert_eq!(bit_reverse(0b1011, 4), 0b1101);
    }

    // --------------------------------------------------------
    // Encoder Tests
    // --------------------------------------------------------

    #[test]
    fn test_encoder_output_length() {
        let enc = NrPolarEncoder::new_pdcch(20, 64);
        assert_eq!(enc.encode(&vec![false; 20]).len(), 64);
    }

    #[test]
    fn test_encoder_deterministic() {
        let enc = NrPolarEncoder::new_pdcch(16, 64);
        let p: Vec<bool> = (0..16).map(|i| i%2==0).collect();
        assert_eq!(enc.encode(&p), enc.encode(&p));
    }

    #[test]
    fn test_encoder_distinct_payloads() {
        let enc = NrPolarEncoder::new_pdcch(16, 64);
        assert_ne!(enc.encode(&vec![false;16]), enc.encode(&vec![true;16]));
    }

    #[test]
    fn test_encoder_pucch_crc_selection() {
        assert_eq!(NrPolarEncoder::new_pucch(25, 64).crc, CrcType::Crc11);
        assert_eq!(NrPolarEncoder::new_pucch(15, 64).crc, CrcType::Crc6);
    }

    // --------------------------------------------------------
    // SC Decoder Roundtrip Tests
    // --------------------------------------------------------

    #[test]
    fn test_sc_roundtrip_n32_k16() {
        let n = 32; let k = 16;
        let pos = info_bit_positions(n, k);
        let payload: Vec<bool> = (0..k).map(|i| i%4<2).collect();
        let mut u = vec![false; n];
        for (i,&p) in pos.iter().enumerate() { u[p] = payload[i]; }
        polar_transform(&mut u);
        let llrs = bits_to_llrs(&u, 18.0);
        assert_eq!(ScDecoder::new(n,k).decode(&llrs), payload);
    }

    #[test]
    fn test_sc_roundtrip_n64_k20() {
        let n = 64; let k = 20;
        let pos = info_bit_positions(n, k);
        let payload: Vec<bool> = (0..k).map(|i| i%3==0).collect();
        let mut u = vec![false; n];
        for (i,&p) in pos.iter().enumerate() { u[p] = payload[i]; }
        polar_transform(&mut u);
        assert_eq!(ScDecoder::new(n,k).decode(&bits_to_llrs(&u, 20.0)), payload);
    }

    #[test]
    fn test_sc_roundtrip_n128_k40() {
        let n = 128; let k = 40;
        let pos = info_bit_positions(n, k);
        let payload: Vec<bool> = (0..k).map(|i| i%7<3).collect();
        let mut u = vec![false; n];
        for (i,&p) in pos.iter().enumerate() { u[p] = payload[i]; }
        polar_transform(&mut u);
        assert_eq!(ScDecoder::new(n,k).decode(&bits_to_llrs(&u, 15.0)), payload);
    }

    // --------------------------------------------------------
    // SCL Decoder Tests
    // --------------------------------------------------------

    #[test]
    fn test_scl_l1_matches_sc() {
        let n = 64; let k = 24;
        let pos = info_bit_positions(n, k);
        let payload: Vec<bool> = (0..k).map(|i| i%5<2).collect();
        let mut u = vec![false; n];
        for (i,&p) in pos.iter().enumerate() { u[p] = payload[i]; }
        polar_transform(&mut u);
        let llrs = bits_to_llrs(&u, 12.0);
        let sc = ScDecoder::new(n,k).decode(&llrs);
        let scl = SclDecoder::new(n,k,1,None).decode(&llrs);
        assert_eq!(sc, scl, "SCL L=1 should match SC");
    }

    #[test]
    fn test_scl_l4_roundtrip() {
        let n = 64; let k = 20;
        let pos = info_bit_positions(n, k);
        let payload: Vec<bool> = (0..k).map(|i| i%2==1).collect();
        let mut u = vec![false; n];
        for (i,&p) in pos.iter().enumerate() { u[p] = payload[i]; }
        polar_transform(&mut u);
        assert_eq!(SclDecoder::new(n,k,4,None).decode(&bits_to_llrs(&u,10.0)), payload);
    }

    #[test]
    fn test_scl_l8_roundtrip() {
        let n = 128; let k = 32;
        let pos = info_bit_positions(n, k);
        let payload: Vec<bool> = (0..k).map(|i| (i/4)%2==0).collect();
        let mut u = vec![false; n];
        for (i,&p) in pos.iter().enumerate() { u[p] = payload[i]; }
        polar_transform(&mut u);
        assert_eq!(SclDecoder::new(n,k,8,None).decode(&bits_to_llrs(&u,10.0)), payload);
    }

    // --------------------------------------------------------
    // Sub-Block Interleaver Tests
    // --------------------------------------------------------

    #[test]
    fn test_subblock_length_preserved() {
        for &n in &[32, 64, 128, 256, 512] {
            assert_eq!(subblock_interleave(&vec![false; n]).len(), n);
        }
    }

    #[test]
    fn test_subblock_roundtrip_n64() {
        let n = 64;
        let bits: Vec<bool> = (0..n).map(|i| i%7<3).collect();
        assert_eq!(subblock_deinterleave(&subblock_interleave(&bits), n), bits);
    }

    #[test]
    fn test_subblock_roundtrip_n128() {
        let n = 128;
        let bits: Vec<bool> = (0..n).map(|i| (i*17+3)%5<2).collect();
        assert_eq!(subblock_deinterleave(&subblock_interleave(&bits), n), bits);
    }

    #[test]
    fn test_subblock_roundtrip_n256() {
        let n = 256;
        let bits: Vec<bool> = (0..n).map(|i| i%3<1).collect();
        assert_eq!(subblock_deinterleave(&subblock_interleave(&bits), n), bits);
    }

    // --------------------------------------------------------
    // Rate Matching Tests
    // --------------------------------------------------------

    #[test]
    fn test_rate_match_repetition_length() {
        assert_eq!(rate_match(&vec![false;64], 128, RateMatchMode::Repetition).len(), 128);
    }

    #[test]
    fn test_rate_match_puncturing_length() {
        assert_eq!(rate_match(&vec![false;128], 64, RateMatchMode::Puncturing).len(), 64);
    }

    #[test]
    fn test_rate_match_shortening_length() {
        assert_eq!(rate_match(&vec![false;128], 80, RateMatchMode::Shortening).len(), 80);
    }

    #[test]
    fn test_rate_match_repetition_content() {
        let n = 32;
        let cw: Vec<bool> = (0..n).map(|i| i%3==0).collect();
        let out = rate_match(&cw, n*2, RateMatchMode::Repetition);
        let y = subblock_interleave(&cw);
        assert_eq!(&out[..n], y.as_slice());
        assert_eq!(&out[n..], y.as_slice());
    }

    #[test]
    fn test_derate_match_repetition_combining() {
        let n = 32;
        let cw: Vec<bool> = (0..n).map(|i| i%5<2).collect();
        let rm = rate_match(&cw, n*2, RateMatchMode::Repetition);
        let llrs: Vec<f64> = rm.iter().map(|&b| if b{-5.0}else{5.0}).collect();
        let dr = derate_match(&llrs, n, RateMatchMode::Repetition);
        assert_eq!(dr.len(), n);
        for &l in &dr { assert!(l.abs() >= 9.9, "Combined LLR should be ≥10, got {}", l); }
    }

    #[test]
    fn test_rate_match_mode_auto() {
        assert_eq!(RateMatchMode::auto(128, 64), RateMatchMode::Repetition);
        assert_eq!(RateMatchMode::auto(32, 128), RateMatchMode::Puncturing);
        assert_eq!(RateMatchMode::auto(100, 128), RateMatchMode::Shortening);
    }

    // --------------------------------------------------------
    // Channel Interleaver Tests
    // --------------------------------------------------------

    #[test]
    fn test_channel_interleave_length_preserved() {
        for &e in &[10, 36, 100] {
            assert_eq!(channel_interleave(&vec![false; e]).len(), e);
        }
    }

    #[test]
    fn test_channel_interleave_roundtrip_small() {
        let bits: Vec<bool> = (0..15).map(|i| i%3==0).collect();
        assert_eq!(channel_deinterleave(&channel_interleave(&bits)), bits);
    }

    #[test]
    fn test_channel_interleave_roundtrip_large() {
        let bits: Vec<bool> = (0..120).map(|i| (i*13+7)%11<5).collect();
        assert_eq!(channel_deinterleave(&channel_interleave(&bits)), bits);
    }

    #[test]
    fn test_triangular_t() {
        assert_eq!(triangular_t(1), 1);
        assert_eq!(triangular_t(3), 2);
        assert_eq!(triangular_t(6), 3);
        assert_eq!(triangular_t(7), 4);
        assert_eq!(triangular_t(15), 5);
    }

    // --------------------------------------------------------
    // Code Block Segmentation Tests
    // --------------------------------------------------------

    #[test]
    fn test_segment_single_block() {
        let payload = vec![false; 100];
        let segs = segment_payload(&payload, CrcType::Crc24C);
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].payload, payload);
    }

    #[test]
    fn test_segment_multiple_blocks_coverage() {
        let payload = vec![true; 2000];
        let segs = segment_payload(&payload, CrcType::Crc24C);
        assert!(segs.len() >= 2);
        let total: usize = segs.iter().map(|s| s.payload.len()).sum();
        assert_eq!(total, 2000);
    }

    #[test]
    fn test_segment_crc_valid() {
        let payload: Vec<bool> = (0..300).map(|i| i%7<3).collect();
        for seg in segment_payload(&payload, CrcType::Crc24C) {
            assert_eq!(seg.crc, CrcType::Crc24C.compute(&seg.payload));
        }
    }

    #[test]
    fn test_reassemble_roundtrip() {
        let payload: Vec<bool> = (0..500).map(|i| (i*11)%13<6).collect();
        let segs = segment_payload(&payload, CrcType::Crc24C);
        assert_eq!(reassemble_segments(&segs, CrcType::Crc24C).unwrap(), payload);
    }

    #[test]
    fn test_reassemble_detects_corruption() {
        let payload = vec![false; 200];
        let mut segs = segment_payload(&payload, CrcType::Crc24C);
        segs[0].crc[0] = !segs[0].crc[0];
        assert!(reassemble_segments(&segs, CrcType::Crc24C).is_none());
    }

    // --------------------------------------------------------
    // Parameter Computation Tests
    // --------------------------------------------------------

    #[test]
    fn test_params_pdcch_k() {
        let p = PolarParams::for_pdcch(12, 54);
        assert_eq!(p.k, 36); // 12+24
        assert!(p.n.is_power_of_two() && p.n >= N_MIN && p.n <= 512);
    }

    #[test]
    fn test_params_pucch_crc11() {
        assert_eq!(PolarParams::for_pucch(25, 100).k, 36);
    }

    #[test]
    fn test_params_pucch_crc6() {
        assert_eq!(PolarParams::for_pucch(12, 50).k, 18);
    }

    #[test]
    fn test_params_pbch() {
        let p = PolarParams::for_pbch();
        assert_eq!(p.a, 32); assert_eq!(p.n, 512);
        assert_eq!(p.e, 864); assert_eq!(p.k, 56);
    }

    #[test]
    fn test_compute_n_power_of_two() {
        for k in [16, 32, 48, 64, 100] {
            let n = compute_n(k, 256, 10);
            assert!(n.is_power_of_two(), "N={} not power-of-2", n);
        }
    }

    #[test]
    fn test_compute_n_at_least_nmin() {
        assert!(compute_n(4, 32, 10) >= N_MIN);
    }

    // --------------------------------------------------------
    // Utilities
    // --------------------------------------------------------

    #[test]
    fn test_bytes_to_bits_roundtrip() {
        let bytes = vec![0xAB, 0xCD];
        assert_eq!(bits_to_bytes(&bytes_to_bits(&bytes)), bytes);
    }

    #[test]
    fn test_bits_to_llrs() {
        let bits = vec![true, false, true];
        assert_eq!(bits_to_llrs(&bits, 4.0), vec![-4.0, 4.0, -4.0]);
    }

    // --------------------------------------------------------
    // End-to-End Tests
    // --------------------------------------------------------

    #[test]
    fn test_e2e_pdcch_repetition() {
        let a = 20; let n = 64; let e = 128;
        let payload: Vec<bool> = (0..a).map(|i| i%3<2).collect();
        let enc = NrPolarEncoder::new_pdcch(a, n);
        let rm = enc.encode_and_rate_match(&payload, e, RateMatchMode::Repetition);
        assert_eq!(rm.len(), e);
        let llrs: Vec<f64> = rm.iter().map(|&b| if b{-20.0}else{20.0}).collect();
        let result = NrPolarDecoder::new_pdcch(a, n, 4).decode_rate_matched(&llrs, RateMatchMode::Repetition);
        assert!(result.is_some(), "PDCCH decode failed");
        assert_eq!(result.unwrap(), payload);
    }

    #[test]
    fn test_e2e_pucch_crc11_shortening() {
        let a = 25; let n = 64; let e = 48;
        let payload: Vec<bool> = (0..a).map(|i| i%4==1).collect();
        // Use shortening-aware constructors: the frozen mask must account for E so that
        // the shortened codeword positions (interleaved index >= E) are guaranteed to be 0.
        let enc = NrPolarEncoder::new_pucch_shortened(a, n, e);
        let rm = enc.encode_and_rate_match(&payload, e, RateMatchMode::Shortening);
        let llrs: Vec<f64> = rm.iter().map(|&b| if b{-20.0}else{20.0}).collect();
        let result = NrPolarDecoder::new_pucch_shortened(a, n, e, 4).decode_rate_matched(&llrs, RateMatchMode::Shortening);
        assert!(result.is_some(), "PUCCH CRC-11 decode failed");
        assert_eq!(result.unwrap(), payload);
    }

    #[test]
    fn test_e2e_pucch_crc6() {
        let a = 6; let n = 32; let e = 32;
        let payload = vec![true, false, true, false, true, false];
        let enc = NrPolarEncoder::new_pucch(a, n);
        let rm = enc.encode_and_rate_match(&payload, e, RateMatchMode::Repetition);
        let llrs: Vec<f64> = rm.iter().map(|&b| if b{-15.0}else{15.0}).collect();
        let result = NrPolarDecoder::new_pucch(a, n, 4).decode_rate_matched(&llrs, RateMatchMode::Repetition);
        assert!(result.is_some(), "PUCCH CRC-6 decode failed");
        assert_eq!(result.unwrap(), payload);
    }

    #[test]
    fn test_e2e_rate_match_all_modes_produce_correct_lengths() {
        let enc = NrPolarEncoder::new_pdcch(30, 128);
        let payload = vec![false; 30];
        for &(e, mode) in &[
            (256, RateMatchMode::Repetition),
            (64,  RateMatchMode::Puncturing),
            (80,  RateMatchMode::Shortening),
        ] {
            assert_eq!(enc.encode_and_rate_match(&payload, e, mode).len(), e);
        }
    }

    #[test]
    fn test_q_sequence_is_permutation() {
        let q = compute_q_sequence();
        let mut seen = [false; 1024];
        for &v in q.iter() { seen[v as usize] = true; }
        assert!(seen.iter().all(|&s| s));
    }
}
