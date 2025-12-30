//! Pulse Position Modulation (PPM) waveform
//!
//! PPM encodes data by the position of pulses within a symbol period.
//! This implementation focuses on the ADS-B variant used in aviation.

use crate::types::IQSample;

use super::adsb::AdsbMessage;
use super::{CommonParams, DemodResult, Waveform, WaveformInfo};

/// Unpack bytes to individual bits (MSB first)
fn bytes_to_bits(data: &[u8]) -> Vec<u8> {
    let mut bits = Vec::with_capacity(data.len() * 8);
    for byte in data {
        for i in (0..8).rev() {
            bits.push((byte >> i) & 1);
        }
    }
    bits
}

/// Pack individual bits to bytes (MSB first)
fn bits_to_bytes(bits: &[u8]) -> Vec<u8> {
    bits.chunks(8)
        .map(|chunk| {
            chunk.iter()
                .enumerate()
                .fold(0u8, |acc, (i, &bit)| {
                    acc | ((bit & 1) << (7 - i))
                })
        })
        .collect()
}

/// Check if data is packed bytes (contains values > 1)
fn is_packed_bytes(data: &[u8]) -> bool {
    data.iter().any(|&b| b > 1)
}

/// PPM modulation types
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PpmVariant {
    /// Standard PPM - pulse position within slot
    Standard,
    /// ADS-B Mode S - Manchester-like (two chips per bit)
    AdsB,
}

/// Pulse Position Modulation waveform
#[derive(Debug, Clone)]
pub struct PPM {
    /// Common waveform parameters
    common: CommonParams,
    /// Symbol rate in symbols per second
    pub symbol_rate: f64,
    /// PPM variant
    pub variant: PpmVariant,
}

impl PPM {
    /// Create a new PPM modulator
    pub fn new(sample_rate: f64, symbol_rate: f64, variant: PpmVariant) -> Self {
        Self {
            common: CommonParams {
                sample_rate,
                carrier_freq: 0.0,
                amplitude: 1.0,
            },
            symbol_rate,
            variant,
        }
    }

    /// Create ADS-B PPM (1 Mbps, Mode S encoding)
    pub fn adsb(sample_rate: f64) -> Self {
        Self::new(sample_rate, 1_000_000.0, PpmVariant::AdsB)
    }

    /// Samples per symbol (minimum depends on variant)
    fn sps(&self) -> usize {
        if self.symbol_rate <= 0.0 {
            return 2;
        }
        let sps = (self.common.sample_rate / self.symbol_rate).round() as usize;
        // ADS-B uses Manchester encoding which needs at least 2 samples per symbol
        let min_sps = match self.variant {
            PpmVariant::AdsB => 2,
            PpmVariant::Standard => 1,
        };
        sps.max(min_sps)
    }

    /// Generate preamble for ADS-B (8µs = 8 bits at 1 Mbps)
    /// Pattern: 1010000101000000 (but we'll generate the pulse pattern)
    fn generate_adsb_preamble(&self) -> Vec<IQSample> {
        let sps = self.sps();
        let half_sps = sps / 2;
        let amp = self.common.amplitude;

        // ADS-B preamble is 8µs with specific pulse pattern
        // Two 0.5µs pulses at 0µs and 1µs, then two at 3.5µs and 4.5µs
        let mut samples = vec![IQSample::new(0.0, 0.0); sps * 8];

        // Pulse at 0-0.5µs
        for i in 0..half_sps {
            samples[i] = IQSample::new(amp, 0.0);
        }
        // Pulse at 1-1.5µs
        for i in sps..sps + half_sps {
            samples[i] = IQSample::new(amp, 0.0);
        }
        // Pulse at 3.5-4µs
        let start = (3.5 * sps as f64) as usize;
        for i in start..start + half_sps {
            if i < samples.len() {
                samples[i] = IQSample::new(amp, 0.0);
            }
        }
        // Pulse at 4.5-5µs
        let start = (4.5 * sps as f64) as usize;
        for i in start..start + half_sps {
            if i < samples.len() {
                samples[i] = IQSample::new(amp, 0.0);
            }
        }

        samples
    }

    /// Generate one bit for ADS-B PPM
    /// Bit 1: high chip (0.5µs) then low chip (0.5µs)
    /// Bit 0: low chip (0.5µs) then high chip (0.5µs)
    fn generate_adsb_bit(&self, bit: u8) -> Vec<IQSample> {
        let sps = self.sps();
        let half_sps = sps / 2;
        let amp = self.common.amplitude;
        let mut samples = vec![IQSample::new(0.0, 0.0); sps];

        if bit == 1 {
            // High then low (Manchester '1')
            for i in 0..half_sps {
                samples[i] = IQSample::new(amp, 0.0);
            }
        } else {
            // Low then high (Manchester '0')
            for i in half_sps..sps {
                samples[i] = IQSample::new(amp, 0.0);
            }
        }

        samples
    }

    /// Demodulate ADS-B PPM signal
    fn demod_adsb(&self, samples: &[IQSample]) -> Vec<u8> {
        let sps = self.sps();
        let half_sps = sps / 2;
        let mut bits = Vec::new();

        // Skip 8-bit preamble
        let data_start = sps * 8;

        for bit_idx in 0..(samples.len().saturating_sub(data_start)) / sps {
            let start = data_start + bit_idx * sps;
            if start + sps > samples.len() {
                break;
            }

            // Measure energy in first half vs second half
            let first_half_energy: f64 = samples[start..start + half_sps]
                .iter()
                .map(|s| s.re * s.re + s.im * s.im)
                .sum();
            let second_half_energy: f64 = samples[start + half_sps..start + sps]
                .iter()
                .map(|s| s.re * s.re + s.im * s.im)
                .sum();

            // Bit 1 has energy in first half, Bit 0 has energy in second half
            bits.push(if first_half_energy > second_half_energy { 1 } else { 0 });
        }

        bits
    }

    /// Demodulate I/Q samples and decode as ADS-B message
    ///
    /// Returns decoded message if valid, None otherwise
    pub fn demodulate_adsb_message(&self, samples: &[IQSample]) -> Option<AdsbMessage> {
        if self.variant != PpmVariant::AdsB {
            return None;
        }

        let bits = self.demod_adsb(samples);
        AdsbMessage::from_bits(&bits)
    }

    /// Detect ADS-B preambles in a continuous sample stream
    ///
    /// Returns sample indices where preambles were detected.
    /// Use these offsets to extract and decode individual messages.
    pub fn detect_preambles(&self, samples: &[IQSample]) -> Vec<usize> {
        if self.variant != PpmVariant::AdsB {
            return Vec::new();
        }

        let sps = self.sps();
        let preamble_len = sps * 8; // 8µs preamble
        let message_len = preamble_len + sps * 112; // preamble + 112 data bits

        if samples.len() < message_len {
            return Vec::new();
        }

        let mut detections = Vec::new();

        // Compute magnitude for correlation
        let magnitudes: Vec<f64> = samples
            .iter()
            .map(|s| (s.re * s.re + s.im * s.im).sqrt())
            .collect();

        // Sliding window preamble detection - find peak correlations
        let mut i = 0;
        while i < samples.len().saturating_sub(message_len) {
            let score = self.preamble_score(&magnitudes[i..i + preamble_len], sps);
            if score > 0.5 {
                // Found a candidate - search nearby for the best match
                let search_start = i;
                let search_end = (i + sps).min(samples.len().saturating_sub(message_len));
                let mut best_idx = i;
                let mut best_score = score;

                for j in search_start..search_end {
                    let s = self.preamble_score(&magnitudes[j..j + preamble_len], sps);
                    if s > best_score {
                        best_score = s;
                        best_idx = j;
                    }
                }

                // Check we're not too close to a previous detection
                if detections.last().map_or(true, |&last| best_idx > last + message_len / 2) {
                    detections.push(best_idx);
                }

                // Skip past this message
                i = best_idx + message_len / 2;
            } else {
                i += 1;
            }
        }

        detections
    }

    /// Calculate preamble correlation score (0.0 to 1.0)
    fn preamble_score(&self, mags: &[f64], sps: usize) -> f64 {
        // Preamble pattern: pulses at 0, 1, 3.5, 4.5 µs
        // Each pulse is 0.5µs wide
        let half_sps = sps / 2;

        // Expected pulse positions (in samples)
        let pulse_positions = [
            0,                           // 0 µs
            sps,                         // 1 µs
            (3.5 * sps as f64) as usize, // 3.5 µs
            (4.5 * sps as f64) as usize, // 4.5 µs
        ];

        // Expected quiet positions (between pulses)
        let quiet_positions = [
            (0.5 * sps as f64) as usize, // 0.5 µs
            (1.5 * sps as f64) as usize, // 1.5 µs
            (2.5 * sps as f64) as usize, // 2.5 µs
            (5.0 * sps as f64) as usize, // 5.0 µs
            (6.0 * sps as f64) as usize, // 6.0 µs
            (7.0 * sps as f64) as usize, // 7.0 µs
        ];

        // Calculate individual pulse magnitudes - ALL must have energy
        let pulse_mags: Vec<f64> = pulse_positions
            .iter()
            .filter_map(|&pos| {
                if pos + half_sps <= mags.len() {
                    Some(
                        mags[pos..pos + half_sps]
                            .iter()
                            .sum::<f64>()
                            / half_sps as f64,
                    )
                } else {
                    None
                }
            })
            .collect();

        // All 4 pulse positions must have energy above threshold
        let min_pulse = 0.1; // Minimum required pulse magnitude
        if pulse_mags.len() < 4 || pulse_mags.iter().any(|&m| m < min_pulse) {
            return 0.0;
        }

        let pulse_avg: f64 = pulse_mags.iter().sum::<f64>() / pulse_mags.len() as f64;

        // Calculate average quiet magnitude
        let quiet_sum: f64 = quiet_positions
            .iter()
            .filter_map(|&pos| {
                if pos + half_sps <= mags.len() {
                    Some(
                        mags[pos..pos + half_sps]
                            .iter()
                            .sum::<f64>()
                            / half_sps as f64,
                    )
                } else {
                    None
                }
            })
            .sum();
        let quiet_avg = quiet_sum / quiet_positions.len() as f64;

        // Return normalized score
        let ratio = pulse_avg / (quiet_avg + 0.001);
        (ratio / 10.0).min(1.0) // Normalize: ratio of 10+ = score of 1.0
    }

    /// Process a continuous sample stream and extract all valid ADS-B messages
    ///
    /// This is the main entry point for decoding ADS-B from an SDR receiver.
    pub fn decode_stream(&self, samples: &[IQSample]) -> Vec<AdsbMessage> {
        if self.variant != PpmVariant::AdsB {
            return Vec::new();
        }

        let sps = self.sps();
        let message_len = sps * (8 + 112); // preamble + 112 data bits

        let preambles = self.detect_preambles(samples);
        let mut messages = Vec::new();

        for preamble_idx in preambles {
            if preamble_idx + message_len <= samples.len() {
                let message_samples = &samples[preamble_idx..preamble_idx + message_len];
                if let Some(msg) = self.demodulate_adsb_message(message_samples) {
                    if msg.crc_valid {
                        messages.push(msg);
                    }
                }
            }
        }

        messages
    }
}

impl Waveform for PPM {
    fn info(&self) -> WaveformInfo {
        let (name, full_name) = match self.variant {
            PpmVariant::Standard => ("PPM", "Pulse Position Modulation"),
            PpmVariant::AdsB => ("ADS-B", "Automatic Dependent Surveillance-Broadcast"),
        };

        WaveformInfo {
            name,
            full_name,
            description: match self.variant {
                PpmVariant::Standard => "Encodes data in pulse timing within symbol",
                PpmVariant::AdsB => "Aviation transponder signal using Mode S PPM at 1090 MHz",
            },
            complexity: 3,
            bits_per_symbol: 1,
            carries_data: true,
            characteristics: match self.variant {
                PpmVariant::Standard => &[
                    "Pulse position encodes data",
                    "Constant pulse width",
                    "Used in IR remotes, optical links",
                    "Simple envelope detection",
                ],
                PpmVariant::AdsB => &[
                    "Mode S transponder signal",
                    "1090 MHz, 1 Mbps data rate",
                    "Manchester-like chip encoding",
                    "112-bit extended squitter messages",
                    "Broadcasts aircraft position/ID",
                ],
            },
            history: match self.variant {
                PpmVariant::Standard => "PPM originated in early pulse communication systems \
                    of the 1940s. Used extensively in infrared remote controls starting in \
                    the 1980s. The NEC IR protocol (1980s) popularized PPM for consumer \
                    electronics, defining the standard for TV remotes worldwide.",
                PpmVariant::AdsB => "ADS-B evolved from Mode S secondary surveillance radar \
                    introduced in the 1960s. The FAA mandated ADS-B Out by January 2020 for \
                    most US airspace. Based on 1090ES (Extended Squitter) technology, it \
                    enables aircraft to broadcast GPS position, velocity, and identification.",
            },
            modern_usage: match self.variant {
                PpmVariant::Standard => "Ubiquitous in IR remote controls (billions of devices). \
                    Used in fiber optic communications, some industrial telemetry, and hobby \
                    RC (radio control) links. Being displaced by digital protocols in new \
                    applications but legacy is enormous.",
                PpmVariant::AdsB => "Mandatory for most aircraft worldwide. Forms backbone of \
                    modern air traffic control with 500,000+ equipped aircraft. Used by flight \
                    tracking services (FlightAware, FlightRadar24). Ground stations and RTL-SDR \
                    hobbyists decode it globally. UAT (978 MHz) variant used for GA in US.",
            },
        }
    }

    fn common_params(&self) -> &CommonParams {
        &self.common
    }

    fn samples_per_symbol(&self) -> usize {
        self.sps()
    }

    fn modulate(&self, data: &[u8]) -> Vec<IQSample> {
        // Convert packed bytes to individual bits if needed
        let bits = if is_packed_bytes(data) {
            bytes_to_bits(data)
        } else {
            data.to_vec()
        };

        match self.variant {
            PpmVariant::AdsB => {
                let mut samples = self.generate_adsb_preamble();
                for &bit in &bits {
                    samples.extend(self.generate_adsb_bit(bit));
                }
                samples
            }
            PpmVariant::Standard => {
                // Standard PPM - pulse position proportional to data value
                let sps = self.sps();
                let amp = self.common.amplitude;
                let mut samples = Vec::new();

                for &bit in &bits {
                    let mut symbol = vec![IQSample::new(0.0, 0.0); sps];
                    let pulse_width = sps / 4;
                    let pulse_pos = if bit == 1 { sps / 4 } else { sps * 3 / 4 - pulse_width };

                    for i in pulse_pos..pulse_pos + pulse_width {
                        if i < sps {
                            symbol[i] = IQSample::new(amp, 0.0);
                        }
                    }
                    samples.extend(symbol);
                }
                samples
            }
        }
    }

    fn demodulate(&self, samples: &[IQSample]) -> DemodResult {
        let individual_bits = match self.variant {
            PpmVariant::AdsB => self.demod_adsb(samples),
            PpmVariant::Standard => {
                let sps = self.sps();
                let mut bits = Vec::new();

                for chunk in samples.chunks(sps) {
                    if chunk.len() < sps {
                        break;
                    }

                    // Find where the pulse energy is concentrated
                    let first_quarter: f64 = chunk[0..sps / 2]
                        .iter()
                        .map(|s| s.re * s.re + s.im * s.im)
                        .sum();
                    let second_quarter: f64 = chunk[sps / 2..sps]
                        .iter()
                        .map(|s| s.re * s.re + s.im * s.im)
                        .sum();

                    bits.push(if first_quarter > second_quarter { 1 } else { 0 });
                }
                bits
            }
        };

        DemodResult {
            symbols: individual_bits.iter().map(|&b| b as u16).collect(),
            bits: bits_to_bytes(&individual_bits),
            ber_estimate: None,
            snr_estimate: None,
            metadata: std::collections::HashMap::new(),
        }
    }
    // Use default get_visualization from trait
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::waveform::adsb::{DownlinkFormat, TypeCode};

    #[test]
    fn test_adsb_roundtrip() {
        let ppm = PPM::adsb(2_000_000.0); // 2 MHz sample rate
        let data = vec![1, 0, 1, 1, 0, 0, 1, 0];

        let modulated = ppm.modulate(&data);
        let result = ppm.demodulate(&modulated);

        // Demodulate returns packed bytes, so compare against packed input
        assert_eq!(result.bits, bits_to_bytes(&data));
    }

    #[test]
    fn test_standard_ppm_roundtrip() {
        let ppm = PPM::new(100_000.0, 1000.0, PpmVariant::Standard);
        let data = vec![1, 0, 1, 1, 0, 0, 1, 0];

        let modulated = ppm.modulate(&data);
        let result = ppm.demodulate(&modulated);

        // Demodulate returns packed bytes, so compare against packed input
        assert_eq!(result.bits, bits_to_bytes(&data));
    }

    #[test]
    fn test_adsb_message_decode() {
        // Known ADS-B message: 8D4840D6202CC371C32CE0576098
        // Aircraft identification for ICAO 4840D6
        let message_bytes: [u8; 14] = [
            0x8D, 0x48, 0x40, 0xD6, 0x20, 0x2C, 0xC3, 0x71,
            0xC3, 0x2C, 0xE0, 0x57, 0x60, 0x98,
        ];

        // Convert bytes to bits
        let mut bits: Vec<u8> = Vec::with_capacity(112);
        for byte in &message_bytes {
            for i in (0..8).rev() {
                bits.push((byte >> i) & 1);
            }
        }

        // Modulate and demodulate
        let ppm = PPM::adsb(2_000_000.0);
        let modulated = ppm.modulate(&bits);
        let decoded = ppm.demodulate_adsb_message(&modulated);

        assert!(decoded.is_some());
        let msg = decoded.unwrap();

        assert!(msg.crc_valid);
        assert_eq!(msg.icao_address, 0x4840D6);
        assert!(matches!(msg.downlink_format, DownlinkFormat::ExtendedSquitter));
        assert!(matches!(msg.type_code, TypeCode::AircraftIdentification(_)));
    }

    #[test]
    fn test_preamble_detection() {
        let ppm = PPM::adsb(2_000_000.0);

        // Generate a valid message
        let message_bytes: [u8; 14] = [
            0x8D, 0x48, 0x40, 0xD6, 0x20, 0x2C, 0xC3, 0x71,
            0xC3, 0x2C, 0xE0, 0x57, 0x60, 0x98,
        ];

        let mut bits: Vec<u8> = Vec::with_capacity(112);
        for byte in &message_bytes {
            for i in (0..8).rev() {
                bits.push((byte >> i) & 1);
            }
        }

        // Modulate to get samples
        let message_samples = ppm.modulate(&bits);

        // Create a stream with padding before and after the message
        let mut stream = vec![IQSample::new(0.0, 0.0); 1000];
        stream.extend_from_slice(&message_samples);
        stream.extend(vec![IQSample::new(0.0, 0.0); 1000]);

        // Detect preambles
        let preambles = ppm.detect_preambles(&stream);

        // Should find at least one preamble (at offset ~1000)
        assert!(!preambles.is_empty(), "Should detect at least one preamble");

        // The detected preamble should be near the start of our message
        let first_preamble = preambles[0];
        assert!(
            first_preamble >= 900 && first_preamble <= 1100,
            "Preamble should be detected near sample 1000, got {}",
            first_preamble
        );
    }

    #[test]
    fn test_stream_decode() {
        // Use 8 MHz sample rate for more reliable preamble detection
        // (4 samples per bit gives better timing resolution)
        let ppm = PPM::adsb(8_000_000.0);

        // Generate a valid message
        let message_bytes: [u8; 14] = [
            0x8D, 0x48, 0x40, 0xD6, 0x20, 0x2C, 0xC3, 0x71,
            0xC3, 0x2C, 0xE0, 0x57, 0x60, 0x98,
        ];

        let mut bits: Vec<u8> = Vec::with_capacity(112);
        for byte in &message_bytes {
            for i in (0..8).rev() {
                bits.push((byte >> i) & 1);
            }
        }

        // Modulate to get samples
        let message_samples = ppm.modulate(&bits);

        // Create a stream with padding
        let mut stream = vec![IQSample::new(0.0, 0.0); 4000];
        stream.extend_from_slice(&message_samples);
        stream.extend(vec![IQSample::new(0.0, 0.0); 4000]);

        // Decode the stream
        let messages = ppm.decode_stream(&stream);

        // Should find the message
        assert_eq!(messages.len(), 1, "Should decode exactly one message");
        assert!(messages[0].crc_valid, "CRC should be valid");
        assert_eq!(messages[0].icao_address, 0x4840D6, "ICAO address should match");
    }

    #[test]
    fn test_multiple_messages_in_stream() {
        // Use 8 MHz sample rate for reliable detection
        let ppm = PPM::adsb(8_000_000.0);

        // Two different messages
        let msg1: [u8; 14] = [
            0x8D, 0x48, 0x40, 0xD6, 0x20, 0x2C, 0xC3, 0x71,
            0xC3, 0x2C, 0xE0, 0x57, 0x60, 0x98,
        ];
        let msg2: [u8; 14] = [
            0x8D, 0x40, 0x62, 0x1D, 0x58, 0xC3, 0x82, 0xD6,
            0x90, 0xC8, 0xAC, 0x28, 0x63, 0xA7,
        ];

        let to_bits = |bytes: &[u8; 14]| -> Vec<u8> {
            let mut bits = Vec::with_capacity(112);
            for byte in bytes {
                for i in (0..8).rev() {
                    bits.push((byte >> i) & 1);
                }
            }
            bits
        };

        let samples1 = ppm.modulate(&to_bits(&msg1));
        let samples2 = ppm.modulate(&to_bits(&msg2));

        // Create stream with both messages separated by silence
        let mut stream = vec![IQSample::new(0.0, 0.0); 2000];
        stream.extend_from_slice(&samples1);
        stream.extend(vec![IQSample::new(0.0, 0.0); 4000]);
        stream.extend_from_slice(&samples2);
        stream.extend(vec![IQSample::new(0.0, 0.0); 2000]);

        // Decode
        let messages = ppm.decode_stream(&stream);

        // Should find both messages
        assert_eq!(messages.len(), 2, "Should decode two messages");

        let icaos: Vec<u32> = messages.iter().map(|m| m.icao_address).collect();
        assert!(icaos.contains(&0x4840D6));
        assert!(icaos.contains(&0x40621D));
    }
}
