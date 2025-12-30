//! ADS-B (Automatic Dependent Surveillance-Broadcast) Message Decoding
//!
//! Implements decoding of Mode S Extended Squitter (DF17) messages
//! with CRC-24 validation and message type parsing.
//!
//! # Message Structure (112 bits)
//!
//! | Field | Bits   | Size    | Description                    |
//! |-------|--------|---------|--------------------------------|
//! | DF    | 1-5    | 5 bits  | Downlink Format (17 for ADS-B) |
//! | CA    | 6-8    | 3 bits  | Capability                     |
//! | ICAO  | 9-32   | 24 bits | Aircraft address               |
//! | ME    | 33-88  | 56 bits | Message (Type Code + Data)     |
//! | PI    | 89-112 | 24 bits | Parity/Interrogator ID         |
//!
//! # References
//! - ICAO Annex 10, Volume IV
//! - RTCA DO-260B
//! - <https://mode-s.org/1090mhz/>

use std::f64::consts::PI;
use std::fmt;

/// CRC-24 generator polynomial for Mode S
/// G(x) = x^24 + x^23 + x^22 + x^21 + x^20 + x^19 + x^18 + x^17 +
///        x^16 + x^15 + x^14 + x^13 + x^12 + x^10 + x^3 + 1
const CRC24_POLYNOMIAL: u32 = 0x1FFF409;

/// Compute CRC-24 for Mode S message
///
/// The CRC is computed over the first 88 bits (11 bytes) of the message.
/// The result should match the last 24 bits (3 bytes) for a valid message.
pub fn crc24(data: &[u8]) -> u32 {
    let mut crc: u32 = 0;

    for &byte in data.iter().take(11) {
        crc ^= (byte as u32) << 16;
        for _ in 0..8 {
            if crc & 0x800000 != 0 {
                crc = (crc << 1) ^ CRC24_POLYNOMIAL;
            } else {
                crc <<= 1;
            }
        }
    }

    crc & 0xFFFFFF
}

/// Validate a 112-bit ADS-B message using CRC-24
///
/// Returns true if the CRC matches (message is valid)
pub fn validate_crc(message: &[u8; 14]) -> bool {
    let computed = crc24(message);
    let received = ((message[11] as u32) << 16)
        | ((message[12] as u32) << 8)
        | (message[13] as u32);

    computed == received
}

/// ADS-B Downlink Format
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownlinkFormat {
    /// Short air-to-air surveillance (ACAS) - DF0
    ShortAirToAir,
    /// Surveillance altitude reply - DF4
    AltitudeReply,
    /// Surveillance identity reply - DF5
    IdentityReply,
    /// All-call reply - DF11
    AllCallReply,
    /// Long air-to-air surveillance (ACAS) - DF16
    LongAirToAir,
    /// Extended Squitter (ADS-B) - DF17
    ExtendedSquitter,
    /// Extended Squitter (non-transponder) - DF18
    ExtendedSquitterNT,
    /// Military extended squitter - DF19
    MilitaryExtended,
    /// Comm-B altitude reply - DF20
    CommBAltitude,
    /// Comm-B identity reply - DF21
    CommBIdentity,
    /// Comm-D (ELM) - DF24
    CommD,
    /// Unknown format
    Unknown(u8),
}

impl From<u8> for DownlinkFormat {
    fn from(value: u8) -> Self {
        match value {
            0 => Self::ShortAirToAir,
            4 => Self::AltitudeReply,
            5 => Self::IdentityReply,
            11 => Self::AllCallReply,
            16 => Self::LongAirToAir,
            17 => Self::ExtendedSquitter,
            18 => Self::ExtendedSquitterNT,
            19 => Self::MilitaryExtended,
            20 => Self::CommBAltitude,
            21 => Self::CommBIdentity,
            24 => Self::CommD,
            other => Self::Unknown(other),
        }
    }
}

/// ADS-B Type Code (determines message content)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeCode {
    /// Aircraft identification (callsign)
    AircraftIdentification(u8), // TC 1-4
    /// Surface position
    SurfacePosition(u8), // TC 5-8
    /// Airborne position with barometric altitude
    AirbornePositionBaro(u8), // TC 9-18
    /// Airborne velocity
    AirborneVelocity, // TC 19
    /// Airborne position with GNSS altitude
    AirbornePositionGNSS(u8), // TC 20-22
    /// Reserved
    Reserved(u8), // TC 23-27
    /// Aircraft status
    AircraftStatus, // TC 28
    /// Target state and status
    TargetState, // TC 29
    /// Reserved
    Reserved30, // TC 30
    /// Operational status
    OperationalStatus, // TC 31
    /// Unknown
    Unknown(u8),
}

impl From<u8> for TypeCode {
    fn from(tc: u8) -> Self {
        match tc {
            1..=4 => Self::AircraftIdentification(tc),
            5..=8 => Self::SurfacePosition(tc),
            9..=18 => Self::AirbornePositionBaro(tc),
            19 => Self::AirborneVelocity,
            20..=22 => Self::AirbornePositionGNSS(tc),
            23..=27 => Self::Reserved(tc),
            28 => Self::AircraftStatus,
            29 => Self::TargetState,
            30 => Self::Reserved30,
            31 => Self::OperationalStatus,
            other => Self::Unknown(other),
        }
    }
}

/// Aircraft identification category
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AircraftCategory {
    /// No category info
    None,
    /// Light (< 15,500 lbs)
    Light,
    /// Small (15,500 - 75,000 lbs)
    Small,
    /// Large (75,000 - 300,000 lbs)
    Large,
    /// High vortex large (e.g., B757)
    HighVortexLarge,
    /// Heavy (> 300,000 lbs)
    Heavy,
    /// High performance (> 5g, > 400 kts)
    HighPerformance,
    /// Rotorcraft
    Rotorcraft,
    /// Glider/sailplane
    Glider,
    /// Lighter than air
    LighterThanAir,
    /// Parachutist/skydiver
    Parachutist,
    /// Ultralight/hang-glider
    Ultralight,
    /// UAV
    UAV,
    /// Space vehicle
    SpaceVehicle,
    /// Surface emergency vehicle
    SurfaceEmergency,
    /// Surface service vehicle
    SurfaceService,
    /// Ground obstruction
    Obstruction,
    /// Cluster obstacle
    ClusterObstacle,
    /// Line obstacle
    LineObstacle,
    /// Reserved
    Reserved,
}

/// Decoded ADS-B message
#[derive(Debug, Clone)]
pub struct AdsbMessage {
    /// Raw 112-bit message (14 bytes)
    pub raw: [u8; 14],
    /// Downlink format
    pub downlink_format: DownlinkFormat,
    /// Capability
    pub capability: u8,
    /// 24-bit ICAO aircraft address
    pub icao_address: u32,
    /// Type code
    pub type_code: TypeCode,
    /// CRC valid flag
    pub crc_valid: bool,
    /// Decoded message content
    pub content: MessageContent,
}

/// Decoded message content based on type code
#[derive(Debug, Clone)]
pub enum MessageContent {
    /// Aircraft identification (callsign)
    Identification {
        category: AircraftCategory,
        callsign: String,
    },
    /// Airborne position
    AirbornePosition {
        /// Altitude in feet
        altitude: Option<i32>,
        /// CPR latitude (raw)
        cpr_lat: u32,
        /// CPR longitude (raw)
        cpr_lon: u32,
        /// CPR odd/even flag
        cpr_odd: bool,
        /// Surveillance status
        surveillance_status: u8,
        /// Single antenna flag
        single_antenna: bool,
        /// Time flag (for CPR)
        time_flag: bool,
    },
    /// Airborne velocity
    AirborneVelocity {
        /// Subtype (1-4)
        subtype: u8,
        /// Heading in degrees (if available)
        heading: Option<f64>,
        /// Ground speed in knots (if available)
        ground_speed: Option<f64>,
        /// Vertical rate in ft/min (if available)
        vertical_rate: Option<i32>,
        /// Vertical rate source (0=GNSS, 1=Baro)
        vr_source: u8,
    },
    /// Surface position
    SurfacePosition {
        /// Ground speed
        ground_speed: Option<f64>,
        /// Track angle
        track: Option<f64>,
        /// CPR latitude (raw)
        cpr_lat: u32,
        /// CPR longitude (raw)
        cpr_lon: u32,
        /// CPR odd/even flag
        cpr_odd: bool,
    },
    /// Aircraft status (emergency, priority)
    AircraftStatus {
        /// Emergency state
        emergency: u8,
        /// Squawk code
        squawk: u16,
    },
    /// Operational status
    OperationalStatus {
        /// Version
        version: u8,
        /// NIC supplement
        nic_supplement: bool,
        /// NAC position
        nac_p: u8,
        /// Barometric altitude integrity
        baro_alt_integrity: bool,
        /// SIL (Source Integrity Level)
        sil: u8,
    },
    /// Unknown or unsupported message type
    Unknown {
        /// Raw ME field (56 bits / 7 bytes)
        me_data: [u8; 7],
    },
}

impl AdsbMessage {
    /// Decode a 112-bit (14-byte) ADS-B message
    pub fn decode(raw: &[u8; 14]) -> Self {
        let downlink_format = DownlinkFormat::from(raw[0] >> 3);
        let capability = raw[0] & 0x07;

        let icao_address = ((raw[1] as u32) << 16)
            | ((raw[2] as u32) << 8)
            | (raw[3] as u32);

        let crc_valid = validate_crc(raw);

        // Type code is first 5 bits of ME field (byte 4)
        let type_code = TypeCode::from(raw[4] >> 3);

        // Extract 56-bit ME field
        let me_data: [u8; 7] = [
            raw[4], raw[5], raw[6], raw[7],
            raw[8], raw[9], raw[10],
        ];

        let content = Self::decode_content(&me_data, type_code);

        Self {
            raw: *raw,
            downlink_format,
            capability,
            icao_address,
            type_code,
            crc_valid,
            content,
        }
    }

    /// Decode message from bits (112 bits)
    pub fn from_bits(bits: &[u8]) -> Option<Self> {
        if bits.len() < 112 {
            return None;
        }

        // Convert bits to bytes
        let mut raw = [0u8; 14];
        for (i, chunk) in bits.chunks(8).enumerate() {
            if i >= 14 {
                break;
            }
            let mut byte = 0u8;
            for (j, &bit) in chunk.iter().enumerate() {
                if bit == 1 {
                    byte |= 1 << (7 - j);
                }
            }
            raw[i] = byte;
        }

        Some(Self::decode(&raw))
    }

    /// Decode message content based on type code
    fn decode_content(me: &[u8; 7], type_code: TypeCode) -> MessageContent {
        match type_code {
            TypeCode::AircraftIdentification(tc) => {
                Self::decode_identification(me, tc)
            }
            TypeCode::AirbornePositionBaro(tc) => {
                Self::decode_airborne_position(me, tc)
            }
            TypeCode::AirbornePositionGNSS(tc) => {
                Self::decode_airborne_position(me, tc)
            }
            TypeCode::AirborneVelocity => {
                Self::decode_velocity(me)
            }
            TypeCode::SurfacePosition(_tc) => {
                Self::decode_surface_position(me)
            }
            TypeCode::AircraftStatus => {
                Self::decode_aircraft_status(me)
            }
            TypeCode::OperationalStatus => {
                Self::decode_operational_status(me)
            }
            _ => MessageContent::Unknown { me_data: *me },
        }
    }

    /// Decode aircraft identification (callsign)
    fn decode_identification(me: &[u8; 7], tc: u8) -> MessageContent {
        // Category from CA field (bits 6-8 of first byte)
        let ca = me[0] & 0x07;
        let category = Self::decode_category(tc, ca);

        // Callsign is 8 characters, 6 bits each (48 bits total)
        // Starts at bit 9 of ME field
        let chars: Vec<u8> = (0..8)
            .map(|i| {
                let bit_start = 8 + i * 6;
                let byte_idx = bit_start / 8;
                let bit_offset = bit_start % 8;

                // Extract 6 bits across byte boundary
                let raw = if bit_offset <= 2 {
                    (me[byte_idx] >> (2 - bit_offset)) & 0x3F
                } else {
                    let high = (me[byte_idx] << (bit_offset - 2)) & 0x3F;
                    let low = me[byte_idx + 1] >> (10 - bit_offset);
                    high | low
                };
                raw
            })
            .collect();

        // Convert to ASCII (ADS-B character set)
        let callsign: String = chars
            .iter()
            .map(|&c| Self::adsb_char(c))
            .collect::<String>()
            .trim()
            .to_string();

        MessageContent::Identification { category, callsign }
    }

    /// Decode aircraft category
    fn decode_category(tc: u8, ca: u8) -> AircraftCategory {
        match (tc, ca) {
            (1, _) => AircraftCategory::Reserved,
            (2, 0) => AircraftCategory::None,
            (2, 1) => AircraftCategory::SurfaceEmergency,
            (2, 3) => AircraftCategory::SurfaceService,
            (2, 4..=7) => AircraftCategory::Obstruction,
            (3, 0) => AircraftCategory::None,
            (3, 1) => AircraftCategory::Glider,
            (3, 2) => AircraftCategory::LighterThanAir,
            (3, 3) => AircraftCategory::Parachutist,
            (3, 4) => AircraftCategory::Ultralight,
            (3, 5) => AircraftCategory::Reserved,
            (3, 6) => AircraftCategory::UAV,
            (3, 7) => AircraftCategory::SpaceVehicle,
            (4, 0) => AircraftCategory::None,
            (4, 1) => AircraftCategory::Light,
            (4, 2) => AircraftCategory::Small,
            (4, 3) => AircraftCategory::Large,
            (4, 4) => AircraftCategory::HighVortexLarge,
            (4, 5) => AircraftCategory::Heavy,
            (4, 6) => AircraftCategory::HighPerformance,
            (4, 7) => AircraftCategory::Rotorcraft,
            _ => AircraftCategory::None,
        }
    }

    /// Convert ADS-B 6-bit character to ASCII
    fn adsb_char(c: u8) -> char {
        match c {
            0 => ' ',
            1..=26 => (b'A' + c - 1) as char,
            48..=57 => (b'0' + c - 48) as char,
            _ => ' ',
        }
    }

    /// Decode airborne position
    fn decode_airborne_position(me: &[u8; 7], tc: u8) -> MessageContent {
        // Surveillance status (bits 6-7)
        let surveillance_status = (me[0] >> 1) & 0x03;

        // Single antenna flag (bit 8)
        let single_antenna = (me[0] & 0x01) == 1;

        // Altitude (bits 9-20) - depends on type code
        let alt_bits = ((me[1] as u32) << 4) | ((me[2] as u32) >> 4);
        let altitude = Self::decode_altitude(alt_bits, tc);

        // Time flag (bit 21)
        let time_flag = (me[2] & 0x08) != 0;

        // CPR odd/even flag (bit 22)
        let cpr_odd = (me[2] & 0x04) != 0;

        // CPR latitude (bits 23-39) - 17 bits
        let cpr_lat = (((me[2] & 0x03) as u32) << 15)
            | ((me[3] as u32) << 7)
            | ((me[4] as u32) >> 1);

        // CPR longitude (bits 40-56) - 17 bits
        let cpr_lon = (((me[4] & 0x01) as u32) << 16)
            | ((me[5] as u32) << 8)
            | (me[6] as u32);

        MessageContent::AirbornePosition {
            altitude,
            cpr_lat,
            cpr_lon,
            cpr_odd,
            surveillance_status,
            single_antenna,
            time_flag,
        }
    }

    /// Decode altitude from 12-bit field
    fn decode_altitude(bits: u32, _tc: u8) -> Option<i32> {
        if bits == 0 {
            return None;
        }

        // Q-bit determines 25ft or 100ft resolution
        let q_bit = (bits >> 4) & 1;

        if q_bit == 1 {
            // 25-foot resolution
            let n = ((bits >> 5) << 4) | (bits & 0x0F);
            Some((n as i32 * 25) - 1000)
        } else {
            // 100-foot resolution (Gillham code)
            // Simplified: just use the raw value
            let n = bits & 0x7FF;
            if n > 0 {
                Some((n as i32 * 100) - 1000)
            } else {
                None
            }
        }
    }

    /// Decode airborne velocity
    fn decode_velocity(me: &[u8; 7]) -> MessageContent {
        let subtype = me[0] & 0x07;

        // Vertical rate source (bit 36)
        let vr_source = (me[4] >> 4) & 0x01;

        // Vertical rate (bits 37-45)
        let vr_sign = (me[4] >> 3) & 0x01;
        let vr_raw = (((me[4] & 0x07) as u32) << 6) | ((me[5] as u32) >> 2);
        let vertical_rate = if vr_raw > 0 {
            let vr = ((vr_raw as i32) - 1) * 64;
            Some(if vr_sign == 1 { -vr } else { vr })
        } else {
            None
        };

        // Heading and speed depend on subtype
        let (heading, ground_speed) = if subtype == 1 || subtype == 2 {
            // Ground speed (subtype 1-2)
            Self::decode_ground_velocity(me, subtype)
        } else if subtype == 3 || subtype == 4 {
            // Airspeed (subtype 3-4)
            Self::decode_airspeed(me, subtype)
        } else {
            (None, None)
        };

        MessageContent::AirborneVelocity {
            subtype,
            heading,
            ground_speed,
            vertical_rate,
            vr_source,
        }
    }

    /// Decode ground velocity (subtype 1-2)
    fn decode_ground_velocity(me: &[u8; 7], _subtype: u8) -> (Option<f64>, Option<f64>) {
        // East-West velocity (bits 14-23)
        let ew_dir = (me[1] >> 2) & 0x01;
        let ew_vel = (((me[1] & 0x03) as u32) << 8) | (me[2] as u32);

        // North-South velocity (bits 25-34)
        let ns_dir = (me[3] >> 7) & 0x01;
        let ns_vel = (((me[3] & 0x7F) as u32) << 3) | ((me[4] as u32) >> 5);

        if ew_vel == 0 || ns_vel == 0 {
            return (None, None);
        }

        let vew = if ew_dir == 1 {
            -(ew_vel as f64 - 1.0)
        } else {
            ew_vel as f64 - 1.0
        };

        let vns = if ns_dir == 1 {
            -(ns_vel as f64 - 1.0)
        } else {
            ns_vel as f64 - 1.0
        };

        let ground_speed = (vew * vew + vns * vns).sqrt();
        let heading = vew.atan2(vns).to_degrees();
        let heading = if heading < 0.0 { heading + 360.0 } else { heading };

        (Some(heading), Some(ground_speed))
    }

    /// Decode airspeed (subtype 3-4)
    fn decode_airspeed(me: &[u8; 7], _subtype: u8) -> (Option<f64>, Option<f64>) {
        // Heading status (bit 14)
        let heading_status = (me[1] >> 2) & 0x01;

        if heading_status == 0 {
            return (None, None);
        }

        // Heading (bits 15-24) - 10 bits, 360/1024 degrees
        let heading_raw = (((me[1] & 0x03) as u32) << 8) | (me[2] as u32);
        let heading = (heading_raw as f64) * 360.0 / 1024.0;

        // Airspeed (bits 26-35)
        let as_raw = (((me[3] & 0x7F) as u32) << 3) | ((me[4] as u32) >> 5);
        let airspeed = if as_raw > 0 {
            Some(as_raw as f64 - 1.0)
        } else {
            None
        };

        (Some(heading), airspeed)
    }

    /// Decode surface position
    fn decode_surface_position(me: &[u8; 7]) -> MessageContent {
        // Ground speed (bits 6-12)
        let mov = ((me[0] & 0x07) << 4) | (me[1] >> 4);
        let ground_speed = Self::decode_surface_speed(mov);

        // Track status (bit 13)
        let track_status = (me[1] >> 3) & 0x01;

        // Track angle (bits 14-20) - 7 bits
        let track_raw = ((me[1] & 0x07) << 4) | (me[2] >> 4);
        let track = if track_status == 1 {
            Some((track_raw as f64) * 360.0 / 128.0)
        } else {
            None
        };

        // CPR odd/even flag (bit 22)
        let cpr_odd = (me[2] & 0x04) != 0;

        // CPR latitude (bits 23-39)
        let cpr_lat = (((me[2] & 0x03) as u32) << 15)
            | ((me[3] as u32) << 7)
            | ((me[4] as u32) >> 1);

        // CPR longitude (bits 40-56)
        let cpr_lon = (((me[4] & 0x01) as u32) << 16)
            | ((me[5] as u32) << 8)
            | (me[6] as u32);

        MessageContent::SurfacePosition {
            ground_speed,
            track,
            cpr_lat,
            cpr_lon,
            cpr_odd,
        }
    }

    /// Decode surface movement/speed
    fn decode_surface_speed(mov: u8) -> Option<f64> {
        match mov {
            0 => None, // Not available
            1 => Some(0.0), // Stopped
            2..=8 => Some(0.125 * (mov as f64 - 1.0)),
            9..=12 => Some(1.0 + 0.25 * (mov as f64 - 9.0)),
            13..=38 => Some(2.0 + 0.5 * (mov as f64 - 13.0)),
            39..=93 => Some(15.0 + (mov as f64 - 39.0)),
            94..=108 => Some(70.0 + 2.0 * (mov as f64 - 94.0)),
            109..=123 => Some(100.0 + 5.0 * (mov as f64 - 109.0)),
            124 => Some(175.0), // >= 175 kts
            _ => None,
        }
    }

    /// Decode aircraft status
    fn decode_aircraft_status(me: &[u8; 7]) -> MessageContent {
        // Emergency state (bits 6-8)
        let emergency = (me[0] >> 0) & 0x07;

        // Squawk code (bits 14-26) - Mode A identity code
        let a = ((me[1] >> 4) & 0x07) as u16;
        let b = ((me[1] >> 1) & 0x07) as u16;
        let c = (((me[1] & 0x01) << 2) | (me[2] >> 6)) as u16;
        let d = ((me[2] >> 3) & 0x07) as u16;
        let squawk = a * 1000 + b * 100 + c * 10 + d;

        MessageContent::AircraftStatus { emergency, squawk }
    }

    /// Decode operational status
    fn decode_operational_status(me: &[u8; 7]) -> MessageContent {
        // Subtype (bits 6-8) - reserved for future use
        let _subtype = me[0] & 0x07;

        // Version (bits 41-43)
        let version = (me[5] >> 5) & 0x07;

        // NIC supplement (bit 44)
        let nic_supplement = (me[5] >> 4) & 0x01 == 1;

        // NAC-p (bits 45-48)
        let nac_p = me[5] & 0x0F;

        // Barometric altitude integrity (bit 53)
        let baro_alt_integrity = (me[6] >> 3) & 0x01 == 1;

        // SIL (bits 54-55)
        let sil = (me[6] >> 1) & 0x03;

        MessageContent::OperationalStatus {
            version,
            nic_supplement,
            nac_p,
            baro_alt_integrity,
            sil,
        }
    }

    /// Get ICAO address as hex string
    pub fn icao_hex(&self) -> String {
        format!("{:06X}", self.icao_address)
    }

    /// Get CPR position data if this is a position message
    pub fn cpr_position(&self) -> Option<CprPosition> {
        match &self.content {
            MessageContent::AirbornePosition {
                cpr_lat,
                cpr_lon,
                cpr_odd,
                altitude,
                ..
            } => Some(CprPosition {
                lat_cpr: *cpr_lat,
                lon_cpr: *cpr_lon,
                odd: *cpr_odd,
                altitude: *altitude,
                surface: false,
            }),
            MessageContent::SurfacePosition {
                cpr_lat,
                cpr_lon,
                cpr_odd,
                ..
            } => Some(CprPosition {
                lat_cpr: *cpr_lat,
                lon_cpr: *cpr_lon,
                odd: *cpr_odd,
                altitude: None,
                surface: true,
            }),
            _ => None,
        }
    }
}

/// CPR (Compact Position Reporting) position data
#[derive(Debug, Clone, Copy)]
pub struct CprPosition {
    /// Raw CPR latitude (17 bits)
    pub lat_cpr: u32,
    /// Raw CPR longitude (17 bits)
    pub lon_cpr: u32,
    /// Odd (true) or even (false) message
    pub odd: bool,
    /// Altitude in feet (if airborne)
    pub altitude: Option<i32>,
    /// Surface position flag
    pub surface: bool,
}

/// Decoded geographic position
#[derive(Debug, Clone, Copy)]
pub struct Position {
    /// Latitude in degrees
    pub latitude: f64,
    /// Longitude in degrees
    pub longitude: f64,
    /// Altitude in feet (if available)
    pub altitude: Option<i32>,
}

/// CPR decoder for position resolution
///
/// ADS-B uses Compact Position Reporting which requires either:
/// - Two messages (even and odd) for global decoding
/// - One message + reference position for local decoding
#[derive(Debug, Default)]
pub struct CprDecoder {
    /// Last even position message
    even: Option<CprPosition>,
    /// Last odd position message
    odd: Option<CprPosition>,
}

impl CprDecoder {
    /// Create a new CPR decoder
    pub fn new() -> Self {
        Self::default()
    }

    /// Process a position message and attempt to decode position
    ///
    /// Returns decoded position if we have both even and odd messages
    pub fn decode(&mut self, cpr: CprPosition) -> Option<Position> {
        if cpr.odd {
            self.odd = Some(cpr);
        } else {
            self.even = Some(cpr);
        }

        // Try global decode if we have both messages
        if let (Some(even), Some(odd)) = (self.even, self.odd) {
            self.decode_global(even, odd)
        } else {
            None
        }
    }

    /// Decode position using reference location (local decode)
    ///
    /// More accurate when aircraft is within 180 NM of reference
    pub fn decode_local(
        &self,
        cpr: CprPosition,
        ref_lat: f64,
        ref_lon: f64,
    ) -> Option<Position> {
        let dlat = if cpr.surface { 90.0 / 60.0 } else { 360.0 / 60.0 };
        let dlon_base = if cpr.surface { 90.0 } else { 360.0 };

        let j = (ref_lat / dlat).floor()
            + ((ref_lat % dlat) / dlat - (cpr.lat_cpr as f64) / 131072.0 + 0.5).floor();
        let lat = dlat * (j + (cpr.lat_cpr as f64) / 131072.0);

        let nl = nl_lat(lat);
        let ni = if cpr.odd { nl - 1.0 } else { nl };
        let ni = ni.max(1.0);
        let dlon = dlon_base / ni;

        let m = (ref_lon / dlon).floor()
            + ((ref_lon % dlon) / dlon - (cpr.lon_cpr as f64) / 131072.0 + 0.5).floor();
        let lon = dlon * (m + (cpr.lon_cpr as f64) / 131072.0);

        Some(Position {
            latitude: lat,
            longitude: normalize_longitude(lon),
            altitude: cpr.altitude,
        })
    }

    /// Decode position using even/odd message pair (global decode)
    fn decode_global(&self, even: CprPosition, odd: CprPosition) -> Option<Position> {
        // CPR latitude/longitude are 17-bit values
        let lat_cpr_even = even.lat_cpr as f64 / 131072.0;
        let lat_cpr_odd = odd.lat_cpr as f64 / 131072.0;
        let lon_cpr_even = even.lon_cpr as f64 / 131072.0;
        let lon_cpr_odd = odd.lon_cpr as f64 / 131072.0;

        // Zone sizes
        let dlat_even = if even.surface { 90.0 / 60.0 } else { 360.0 / 60.0 };
        let dlat_odd = if odd.surface { 90.0 / 59.0 } else { 360.0 / 59.0 };

        // Latitude zone index
        let j = (59.0 * lat_cpr_even - 60.0 * lat_cpr_odd + 0.5).floor();

        // Latitude candidates
        let mut lat_even = dlat_even * (j % 60.0 + lat_cpr_even);
        let mut lat_odd = dlat_odd * (j % 59.0 + lat_cpr_odd);

        // Normalize latitudes to [-90, 90]
        if lat_even >= 270.0 {
            lat_even -= 360.0;
        }
        if lat_odd >= 270.0 {
            lat_odd -= 360.0;
        }

        // Check zone consistency
        let nl_even = nl_lat(lat_even);
        let nl_odd = nl_lat(lat_odd);

        if nl_even != nl_odd {
            // Zone mismatch - positions are too far apart in time
            return None;
        }

        // Use the most recent message for final position
        let (lat, lon_cpr, is_odd) = if odd.odd {
            (lat_odd, lon_cpr_odd, true)
        } else {
            (lat_even, lon_cpr_even, false)
        };

        // Longitude zone count
        let nl = nl_lat(lat);
        let ni = if is_odd {
            (nl - 1.0).max(1.0)
        } else {
            nl.max(1.0)
        };

        let dlon = if even.surface { 90.0 / ni } else { 360.0 / ni };

        // Longitude zone index
        let m = (lon_cpr_even * (nl - 1.0) - lon_cpr_odd * nl + 0.5).floor();

        let lon = dlon * (m % ni + lon_cpr);

        Some(Position {
            latitude: lat,
            longitude: normalize_longitude(lon),
            altitude: odd.altitude.or(even.altitude),
        })
    }

    /// Clear stored positions
    pub fn reset(&mut self) {
        self.even = None;
        self.odd = None;
    }
}

/// Calculate NL (number of longitude zones) for a given latitude
fn nl_lat(lat: f64) -> f64 {
    if lat.abs() >= 87.0 {
        return 1.0;
    }

    let lat_rad = lat.abs() * PI / 180.0;
    let nz = 15.0; // Number of latitude zones (60 for even, 59 for odd)

    let a = 1.0 - (1.0 - (PI / (2.0 * nz)).cos()) / lat_rad.cos().powi(2);

    if a < 0.0 {
        1.0
    } else {
        (2.0 * PI / a.acos()).floor()
    }
}

/// Normalize longitude to [-180, 180]
fn normalize_longitude(lon: f64) -> f64 {
    let mut lon = lon;
    while lon > 180.0 {
        lon -= 360.0;
    }
    while lon < -180.0 {
        lon += 360.0;
    }
    lon
}

impl fmt::Display for AdsbMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "ADS-B [{}] ICAO:{} {:?}",
            if self.crc_valid { "OK" } else { "CRC ERR" },
            self.icao_hex(),
            self.type_code
        )?;

        match &self.content {
            MessageContent::Identification { callsign, category } => {
                write!(f, " Callsign: {} ({:?})", callsign, category)?;
            }
            MessageContent::AirbornePosition {
                altitude,
                cpr_odd,
                ..
            } => {
                if let Some(alt) = altitude {
                    write!(f, " Alt: {} ft", alt)?;
                }
                write!(f, " CPR: {}", if *cpr_odd { "odd" } else { "even" })?;
            }
            MessageContent::AirborneVelocity {
                heading,
                ground_speed,
                vertical_rate,
                ..
            } => {
                if let Some(gs) = ground_speed {
                    write!(f, " GS: {:.0} kts", gs)?;
                }
                if let Some(hdg) = heading {
                    write!(f, " HDG: {:.0}°", hdg)?;
                }
                if let Some(vr) = vertical_rate {
                    write!(f, " VS: {} fpm", vr)?;
                }
            }
            MessageContent::AircraftStatus { squawk, emergency } => {
                write!(f, " Squawk: {:04} Emerg: {}", squawk, emergency)?;
            }
            _ => {}
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_crc24() {
        // Test vector: Valid ADS-B message
        // 8D4840D6202CC371C32CE0576098
        let msg: [u8; 14] = [
            0x8D, 0x48, 0x40, 0xD6, 0x20, 0x2C, 0xC3, 0x71,
            0xC3, 0x2C, 0xE0, 0x57, 0x60, 0x98,
        ];

        assert!(validate_crc(&msg));
    }

    #[test]
    fn test_crc_invalid() {
        // Corrupt message
        let msg: [u8; 14] = [
            0x8D, 0x48, 0x40, 0xD6, 0x20, 0x2C, 0xC3, 0x71,
            0xC3, 0x2C, 0xE0, 0x57, 0x60, 0x99, // Last byte wrong
        ];

        assert!(!validate_crc(&msg));
    }

    #[test]
    fn test_decode_identification() {
        // Example: 8D4840D6202CC371C32CE0576098
        // Aircraft identification: KLM1023
        let msg: [u8; 14] = [
            0x8D, 0x48, 0x40, 0xD6, 0x20, 0x2C, 0xC3, 0x71,
            0xC3, 0x2C, 0xE0, 0x57, 0x60, 0x98,
        ];

        let decoded = AdsbMessage::decode(&msg);

        assert!(decoded.crc_valid);
        assert_eq!(decoded.icao_address, 0x4840D6);
        assert!(matches!(decoded.downlink_format, DownlinkFormat::ExtendedSquitter));

        if let MessageContent::Identification { callsign, .. } = &decoded.content {
            assert!(!callsign.is_empty());
        }
    }

    #[test]
    fn test_decode_position() {
        // Example position message: 8D40621D58C382D690C8AC2863A7
        let msg: [u8; 14] = [
            0x8D, 0x40, 0x62, 0x1D, 0x58, 0xC3, 0x82, 0xD6,
            0x90, 0xC8, 0xAC, 0x28, 0x63, 0xA7,
        ];

        let decoded = AdsbMessage::decode(&msg);

        assert!(decoded.crc_valid);
        assert!(matches!(
            decoded.type_code,
            TypeCode::AirbornePositionBaro(_)
        ));
    }

    #[test]
    fn test_decode_velocity() {
        // Example velocity message: 8D485020994409940838175B284F
        let msg: [u8; 14] = [
            0x8D, 0x48, 0x50, 0x20, 0x99, 0x44, 0x09, 0x94,
            0x08, 0x38, 0x17, 0x5B, 0x28, 0x4F,
        ];

        let decoded = AdsbMessage::decode(&msg);

        assert!(decoded.crc_valid);
        assert!(matches!(decoded.type_code, TypeCode::AirborneVelocity));

        if let MessageContent::AirborneVelocity {
            ground_speed,
            heading,
            ..
        } = &decoded.content
        {
            assert!(ground_speed.is_some());
            assert!(heading.is_some());
        }
    }

    #[test]
    fn test_adsb_char() {
        assert_eq!(AdsbMessage::adsb_char(0), ' ');
        assert_eq!(AdsbMessage::adsb_char(1), 'A');
        assert_eq!(AdsbMessage::adsb_char(26), 'Z');
        assert_eq!(AdsbMessage::adsb_char(48), '0');
        assert_eq!(AdsbMessage::adsb_char(57), '9');
    }

    #[test]
    fn test_cpr_position_extraction() {
        // Position message with CPR data
        let msg: [u8; 14] = [
            0x8D, 0x40, 0x62, 0x1D, 0x58, 0xC3, 0x82, 0xD6,
            0x90, 0xC8, 0xAC, 0x28, 0x63, 0xA7,
        ];

        let decoded = AdsbMessage::decode(&msg);
        let cpr = decoded.cpr_position();

        assert!(cpr.is_some());
        let cpr = cpr.unwrap();
        assert!(cpr.lat_cpr > 0);
        assert!(cpr.lon_cpr > 0);
    }

    #[test]
    fn test_cpr_decoder_needs_both_frames() {
        let mut decoder = CprDecoder::new();

        // Create a mock even frame
        let even = CprPosition {
            lat_cpr: 93000,
            lon_cpr: 51000,
            odd: false,
            altitude: Some(35000),
            surface: false,
        };

        // Single frame should not produce position
        let pos = decoder.decode(even);
        assert!(pos.is_none());

        // Create a mock odd frame (different CPR values)
        let odd = CprPosition {
            lat_cpr: 74158,
            lon_cpr: 50194,
            odd: true,
            altitude: Some(35000),
            surface: false,
        };

        // Now we should get a position
        let pos = decoder.decode(odd);
        assert!(pos.is_some());
    }

    #[test]
    fn test_nl_lat_function() {
        // Test the NL function at various latitudes
        assert_eq!(nl_lat(0.0), 59.0); // Equator
        assert_eq!(nl_lat(87.0), 1.0); // Polar
        assert_eq!(nl_lat(-87.0), 1.0); // Polar south

        // Mid-latitudes should have intermediate values
        let nl_40 = nl_lat(40.0);
        assert!(nl_40 > 1.0 && nl_40 < 59.0);
    }

    #[test]
    fn test_normalize_longitude() {
        assert_eq!(normalize_longitude(0.0), 0.0);
        assert_eq!(normalize_longitude(180.0), 180.0);
        assert_eq!(normalize_longitude(-180.0), -180.0);
        assert_eq!(normalize_longitude(360.0), 0.0);
        assert_eq!(normalize_longitude(-360.0), 0.0);
        assert_eq!(normalize_longitude(270.0), -90.0);
    }
}
