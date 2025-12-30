//! Meshtastic Protocol Implementation
//!
//! This module implements the Meshtastic mesh networking protocol, enabling
//! R4W to participate in the global Meshtastic network with 40,000+ nodes.
//!
//! ## Protocol Overview
//!
//! - Physical Layer: LoRa CSS modulation with 16-symbol preamble, sync word 0x2B
//! - MAC Layer: CSMA/CA with SNR-based contention windows
//! - Routing: Managed flood for broadcasts, next-hop for direct messages
//! - Messages: Protobuf-encoded (Position, Text, NodeInfo, Telemetry, etc.)
//!
//! ## Modem Presets
//!
//! | Preset | SF | BW | CR | Description |
//! |--------|----|----|-----|-------------|
//! | LongFast | 11 | 250 | 4/5 | Long range, higher throughput |
//! | LongSlow | 12 | 125 | 4/8 | Maximum range, lowest throughput |
//! | LongModerate | 11 | 125 | 4/8 | Long range, moderate throughput |
//! | MediumFast | 9 | 250 | 4/5 | Medium range |
//! | MediumSlow | 10 | 250 | 4/5 | Medium range, lower throughput |
//! | ShortFast | 7 | 250 | 4/5 | Short range, highest throughput |
//! | ShortSlow | 8 | 250 | 4/5 | Short range |

#[cfg(feature = "crypto")]
use super::crypto::{ChannelKey, CryptoContext};
use super::mac::{CsmaConfig, MacLayer, TransmitDecision};
use super::neighbor::{Neighbor, NeighborTable, NodeInfo};
use super::packet::{MeshPacket, NodeId, PacketType};
use super::routing::{FloodRouter, NextHopRouter, Route};
use super::telemetry::{DeviceMetrics, EnvironmentMetrics, Telemetry, TelemetryConfig, TelemetryVariant};
use super::traits::{MeshError, MeshNetwork, MeshResult, MeshStats};
use std::time::{Duration, Instant};

/// Meshtastic modem presets
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModemPreset {
    /// Long range, fast: SF11, BW250, CR4/5
    LongFast,
    /// Long range, slow (maximum range): SF12, BW125, CR4/8
    LongSlow,
    /// Long range, moderate: SF11, BW125, CR4/8
    LongModerate,
    /// Medium range, fast: SF9, BW250, CR4/5
    MediumFast,
    /// Medium range, slow: SF10, BW250, CR4/5
    MediumSlow,
    /// Short range, fast (highest throughput): SF7, BW250, CR4/5
    ShortFast,
    /// Short range, slow: SF8, BW250, CR4/5
    ShortSlow,
}

impl ModemPreset {
    /// Get LoRa parameters for this preset
    pub fn lora_params(&self) -> (u8, u32, u8) {
        // Returns (spreading_factor, bandwidth_hz, coding_rate)
        match self {
            ModemPreset::LongFast => (11, 250_000, 5),
            ModemPreset::LongSlow => (12, 125_000, 8),
            ModemPreset::LongModerate => (11, 125_000, 8),
            ModemPreset::MediumFast => (9, 250_000, 5),
            ModemPreset::MediumSlow => (10, 250_000, 5),
            ModemPreset::ShortFast => (7, 250_000, 5),
            ModemPreset::ShortSlow => (8, 250_000, 5),
        }
    }

    /// Get the sync word for Meshtastic (0x2B)
    pub fn sync_word() -> u8 {
        0x2B
    }

    /// Get preamble length (16 symbols)
    pub fn preamble_length() -> u8 {
        16
    }
}

impl Default for ModemPreset {
    fn default() -> Self {
        ModemPreset::LongFast
    }
}

/// Regional frequency configuration
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Region {
    /// US (902-928 MHz)
    US,
    /// EU (863-870 MHz)
    EU,
    /// China (470-510 MHz)
    CN,
    /// Japan (920-925 MHz)
    JP,
    /// Australia/New Zealand (915-928 MHz)
    ANZ,
    /// Korea (920-923 MHz)
    KR,
    /// Taiwan (920-925 MHz)
    TW,
    /// India (865-867 MHz)
    IN,
    /// Unset (use EU defaults)
    Unset,
}

impl Region {
    /// Get frequency range for region
    pub fn frequency_range(&self) -> (u64, u64) {
        match self {
            Region::US => (902_000_000, 928_000_000),
            Region::EU | Region::Unset => (863_000_000, 870_000_000),
            Region::CN => (470_000_000, 510_000_000),
            Region::JP => (920_000_000, 925_000_000),
            Region::ANZ => (915_000_000, 928_000_000),
            Region::KR => (920_000_000, 923_000_000),
            Region::TW => (920_000_000, 925_000_000),
            Region::IN => (865_000_000, 867_000_000),
        }
    }

    /// Get default primary frequency
    pub fn primary_frequency(&self) -> u64 {
        let (start, end) = self.frequency_range();
        (start + end) / 2
    }

    /// Get duty cycle limit (fraction)
    pub fn duty_cycle_limit(&self) -> f32 {
        match self {
            Region::EU => 0.01, // 1% in EU
            _ => 1.0,          // No limit (check local regulations)
        }
    }
}

impl Default for Region {
    fn default() -> Self {
        Region::US
    }
}

/// Channel configuration
#[derive(Debug, Clone)]
pub struct ChannelConfig {
    /// Channel name
    pub name: String,
    /// Pre-shared key (32 bytes, None for unencrypted)
    pub psk: Option<[u8; 32]>,
    /// Modem preset
    pub preset: ModemPreset,
    /// Uplink enabled (MQTT gateway)
    pub uplink_enabled: bool,
    /// Downlink enabled (receive from MQTT)
    pub downlink_enabled: bool,
}

impl ChannelConfig {
    /// Create a new channel configuration
    pub fn new(name: &str, preset: ModemPreset) -> Self {
        Self {
            name: name.to_string(),
            psk: None,
            preset,
            uplink_enabled: false,
            downlink_enabled: false,
        }
    }

    /// Create an encrypted channel with the given PSK
    pub fn with_psk(name: &str, psk: [u8; 32], preset: ModemPreset) -> Self {
        Self {
            name: name.to_string(),
            psk: Some(psk),
            preset,
            uplink_enabled: false,
            downlink_enabled: false,
        }
    }

    /// Create an encrypted channel using the default Meshtastic PSK
    #[cfg(feature = "crypto")]
    pub fn with_default_psk(name: &str, preset: ModemPreset) -> Self {
        use super::crypto::DEFAULT_PSK;
        let mut psk = [0u8; 32];
        // Extend default PSK to 32 bytes
        psk[..DEFAULT_PSK.len()].copy_from_slice(DEFAULT_PSK);
        Self::with_psk(name, psk, preset)
    }

    /// Check if this channel uses encryption
    pub fn is_encrypted(&self) -> bool {
        self.psk.is_some()
    }

    /// Create a CryptoContext for this channel (if encrypted)
    #[cfg(feature = "crypto")]
    pub fn crypto_context(&self) -> Option<CryptoContext> {
        self.psk.map(|psk| {
            let key = ChannelKey::from_raw(psk, &self.name);
            CryptoContext::from_key(key)
        })
    }

    /// Get channel hash for wire format
    pub fn channel_hash(&self) -> u8 {
        #[cfg(feature = "crypto")]
        if let Some(ref ctx) = self.crypto_context() {
            return ctx.channel_hash();
        }
        // Simple hash without crypto feature
        self.name.bytes().fold(0u8, |acc, b| acc.wrapping_add(b))
    }
}

impl Default for ChannelConfig {
    fn default() -> Self {
        Self {
            name: "LongFast".to_string(),
            psk: None, // Default public channel
            preset: ModemPreset::LongFast,
            uplink_enabled: false,
            downlink_enabled: false,
        }
    }
}

/// Meshtastic node configuration
#[derive(Debug, Clone)]
pub struct MeshtasticConfig {
    /// Node ID (random if None)
    pub node_id: Option<NodeId>,
    /// Short name (up to 4 chars)
    pub short_name: String,
    /// Long name (up to 40 chars)
    pub long_name: String,
    /// Hardware model ID
    pub hardware_model: u8,
    /// Region for frequency selection
    pub region: Region,
    /// Primary channel configuration
    pub primary_channel: ChannelConfig,
    /// Secondary channels (up to 7)
    pub secondary_channels: Vec<ChannelConfig>,
    /// Position sharing enabled
    pub position_enabled: bool,
    /// Position update interval (seconds)
    pub position_interval: u64,
    /// Node is a router (longer range settings)
    pub is_router: bool,
    /// Hop limit for broadcasts
    pub hop_limit: u8,
    /// Telemetry configuration
    pub telemetry: TelemetryConfig,
    /// Enable encryption on primary channel
    pub encryption_enabled: bool,
}

impl Default for MeshtasticConfig {
    fn default() -> Self {
        Self {
            node_id: None,
            short_name: "NODE".to_string(),
            long_name: "R4W Node".to_string(),
            hardware_model: 0xFF, // Unknown/custom
            region: Region::default(),
            primary_channel: ChannelConfig::default(),
            secondary_channels: Vec::new(),
            position_enabled: false,
            position_interval: 900, // 15 minutes
            is_router: false,
            hop_limit: 3,
            telemetry: TelemetryConfig::default(),
            encryption_enabled: false,
        }
    }
}

/// Meshtastic mesh node implementation
#[derive(Debug)]
pub struct MeshtasticNode {
    /// Node configuration
    config: MeshtasticConfig,
    /// Our node ID
    node_id: NodeId,
    /// Node information
    node_info: NodeInfo,
    /// Neighbor table
    neighbors: NeighborTable,
    /// Flood router
    flood_router: FloodRouter,
    /// Next-hop router
    next_hop_router: NextHopRouter,
    /// MAC layer
    mac: MacLayer,
    /// Statistics
    stats: MeshStats,
    /// Application-layer receive queue
    rx_queue: Vec<MeshPacket>,
    /// Last position broadcast time
    last_position_broadcast: Option<Instant>,
    /// Last node info broadcast time
    last_nodeinfo_broadcast: Option<Instant>,
    /// Crypto context for primary channel (when encryption enabled)
    #[cfg(feature = "crypto")]
    crypto: Option<CryptoContext>,
    /// Current device metrics
    device_metrics: DeviceMetrics,
    /// Current environment metrics (if sensors available)
    environment_metrics: Option<EnvironmentMetrics>,
    /// Last telemetry broadcast time
    last_telemetry_broadcast: Option<Instant>,
    /// Node start time for uptime calculation
    start_time: Instant,
}

impl MeshtasticNode {
    /// Create a new Meshtastic node
    pub fn new(config: MeshtasticConfig) -> Self {
        let node_id = config.node_id.unwrap_or_else(NodeId::random);

        let node_info = NodeInfo {
            node_id,
            short_name: config.short_name.clone(),
            long_name: config.long_name.clone(),
            hardware_model: config.hardware_model,
            firmware_version: env!("CARGO_PKG_VERSION").to_string(),
            position: None,
            battery_level: None,
            is_router: config.is_router,
        };

        // Configure MAC for Meshtastic
        let mac_config = CsmaConfig {
            cw_min: 16,
            cw_max: 256,
            slot_time_ms: 10,
            max_backoff_attempts: 7,
            difs_ms: 50,
            target_utilization: config.region.duty_cycle_limit(),
            cad_threshold: -115.0,
        };

        // Initialize crypto context if encryption is enabled
        #[cfg(feature = "crypto")]
        let crypto = if config.encryption_enabled {
            config.primary_channel.crypto_context()
        } else {
            None
        };

        Self {
            node_id,
            flood_router: FloodRouter::new(node_id),
            next_hop_router: NextHopRouter::new(node_id),
            mac: MacLayer::new(mac_config),
            neighbors: NeighborTable::new(7200, 256),
            node_info,
            config,
            stats: MeshStats::default(),
            rx_queue: Vec::new(),
            last_position_broadcast: None,
            last_nodeinfo_broadcast: None,
            #[cfg(feature = "crypto")]
            crypto,
            device_metrics: DeviceMetrics::default(),
            environment_metrics: None,
            last_telemetry_broadcast: None,
            start_time: Instant::now(),
        }
    }

    /// Create with default configuration
    pub fn with_defaults() -> Self {
        Self::new(MeshtasticConfig::default())
    }

    /// Get the node's configuration
    pub fn config(&self) -> &MeshtasticConfig {
        &self.config
    }

    /// Set the node's position
    pub fn set_position(&mut self, lat: f64, lon: f64, alt: f32) {
        self.node_info.position = Some((lat, lon, alt));
    }

    /// Set battery level
    pub fn set_battery_level(&mut self, level: u8) {
        self.node_info.battery_level = Some(level.min(100));
        self.device_metrics.battery_level = Some(level.min(100));
    }

    /// Update device metrics
    pub fn update_device_metrics(&mut self) {
        // Update uptime
        self.device_metrics.uptime_seconds = Some(self.start_time.elapsed().as_secs() as u32);

        // Update channel utilization from MAC layer
        self.device_metrics.channel_utilization = Some(self.mac.channel_utilization_cached());
    }

    /// Set voltage reading
    pub fn set_voltage(&mut self, voltage: f32) {
        self.device_metrics.voltage = Some(voltage);
    }

    /// Set air utilization TX percentage
    pub fn set_air_util_tx(&mut self, util: f32) {
        self.device_metrics.air_util_tx = Some(util);
    }

    /// Set environment metrics (for nodes with sensors)
    pub fn set_environment_metrics(&mut self, metrics: EnvironmentMetrics) {
        self.environment_metrics = Some(metrics);
    }

    /// Get current device metrics
    pub fn device_metrics(&self) -> &DeviceMetrics {
        &self.device_metrics
    }

    /// Get current environment metrics
    pub fn environment_metrics(&self) -> Option<&EnvironmentMetrics> {
        self.environment_metrics.as_ref()
    }

    /// Check if encryption is enabled
    pub fn is_encrypted(&self) -> bool {
        #[cfg(feature = "crypto")]
        return self.crypto.is_some();
        #[cfg(not(feature = "crypto"))]
        return false;
    }

    /// Get packets ready for application layer
    pub fn receive(&mut self) -> Vec<MeshPacket> {
        std::mem::take(&mut self.rx_queue)
    }

    /// Get LoRa parameters for current channel
    pub fn lora_params(&self) -> (u8, u32, u8) {
        self.config.primary_channel.preset.lora_params()
    }

    /// Get the primary frequency for current region
    pub fn frequency(&self) -> u64 {
        self.config.region.primary_frequency()
    }

    /// Process channel state and get packet to transmit (if any)
    pub fn process_tx(&mut self, channel_busy: bool) -> Option<Vec<u8>> {
        // Check for pending rebroadcasts
        if let Some(packet) = self.flood_router.get_pending_rebroadcast() {
            if self.queue_packet(&packet).is_ok() {
                self.stats.packets_forwarded += 1;
            }
        }

        // Check MAC layer
        match self.mac.can_transmit(channel_busy) {
            TransmitDecision::TransmitNow => {
                if let Some(packet) = self.mac.start_tx() {
                    self.stats.packets_tx += 1;
                    self.stats.bytes_tx += packet.len() as u64;
                    Some(packet)
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Called when transmission is complete
    pub fn tx_complete(&mut self, duration: Duration) {
        self.mac.tx_complete(duration);
    }

    /// Queue a packet for transmission with optional encryption
    fn queue_packet(&mut self, packet: &MeshPacket) -> MeshResult<()> {
        #[cfg(feature = "crypto")]
        let bytes = if let Some(ref crypto) = self.crypto {
            // Encrypt the packet
            match packet.encrypt(crypto) {
                Ok(encrypted) => encrypted,
                Err(_) => packet.to_bytes(), // Fall back to unencrypted on error
            }
        } else {
            packet.to_bytes()
        };

        #[cfg(not(feature = "crypto"))]
        let bytes = packet.to_bytes();

        self.mac.queue_tx(bytes).map_err(|_| MeshError::QueueFull)
    }

    /// Decrypt a received packet if encryption is enabled
    #[cfg(feature = "crypto")]
    #[allow(dead_code)] // Will be used when processing raw encrypted bytes
    fn decrypt_packet(&self, data: &[u8]) -> Option<MeshPacket> {
        if let Some(ref crypto) = self.crypto {
            // Try to decrypt
            MeshPacket::decrypt(data, crypto).ok()
        } else {
            // No encryption, parse directly
            MeshPacket::from_bytes(data)
        }
    }

    /// Parse a received packet (no crypto support)
    #[cfg(not(feature = "crypto"))]
    #[allow(dead_code)] // Will be used when processing raw bytes
    fn decrypt_packet(&self, data: &[u8]) -> Option<MeshPacket> {
        MeshPacket::from_bytes(data)
    }

    /// Broadcast node info
    fn broadcast_node_info(&mut self) {
        let packet = MeshPacket::node_info(
            self.node_id,
            &self.node_info.short_name,
            &self.node_info.long_name,
        );
        let _ = self.queue_packet(&packet);
        self.last_nodeinfo_broadcast = Some(Instant::now());
    }

    /// Broadcast position
    fn broadcast_position(&mut self) {
        if let Some((lat, lon, alt)) = self.node_info.position {
            let packet = MeshPacket::position(self.node_id, lat, lon, alt);
            let _ = self.queue_packet(&packet);
            self.last_position_broadcast = Some(Instant::now());
        }
    }

    /// Broadcast telemetry
    fn broadcast_telemetry(&mut self) {
        // Update metrics before broadcasting
        self.update_device_metrics();

        let telemetry = Telemetry {
            time: self.start_time.elapsed().as_secs() as u32,
            variant: TelemetryVariant::Device(self.device_metrics.clone()),
        };

        let packet = MeshPacket::telemetry(self.node_id, &telemetry);
        let _ = self.queue_packet(&packet);
        self.last_telemetry_broadcast = Some(Instant::now());
    }
}

impl MeshNetwork for MeshtasticNode {
    fn node_id(&self) -> NodeId {
        self.node_id
    }

    fn discover_neighbors(&mut self) -> Vec<Neighbor> {
        // Broadcast node info to announce ourselves
        let should_broadcast = self.last_nodeinfo_broadcast
            .map(|t| t.elapsed() > Duration::from_secs(900))
            .unwrap_or(true);

        if should_broadcast {
            self.broadcast_node_info();
        }

        // Return current neighbors
        self.neighbors.active().into_iter().cloned().collect()
    }

    fn neighbors(&self) -> &[Neighbor] {
        // Note: This returns empty slice due to ownership constraints
        // Use discover_neighbors() for actual neighbor list
        &[]
    }

    fn route(&self, dest: NodeId) -> Option<Route> {
        // Check if direct neighbor
        if self.neighbors.get(&dest).is_some() {
            return Some(Route::direct(dest));
        }

        // Check routing table
        self.next_hop_router.get_route(&dest).cloned()
    }

    fn forward(&mut self, packet: MeshPacket) -> MeshResult<()> {
        self.queue_packet(&packet)
    }

    fn on_receive(&mut self, mut packet: MeshPacket, rssi: f32, snr: f32) -> Vec<MeshPacket> {
        // Set reception metadata
        packet.set_rx_metadata(rssi, snr);

        // Update neighbor table
        self.neighbors.update(packet.header.source, rssi, snr);

        // Update routing table from overheard packet
        if let Some(neighbor) = self.neighbors.get(&packet.header.source) {
            self.next_hop_router.learn_route(
                &packet,
                packet.header.source,
                neighbor.link_quality.quality_score(),
            );
        }

        // Process with flood router
        let (local, _rebroadcast) = self.flood_router.process_incoming(packet.clone(), rssi, snr);

        // Update stats
        self.stats.packets_rx += 1;
        self.stats.bytes_rx += packet.to_bytes().len() as u64;

        // Handle specific packet types
        if let Some(ref local_packet) = local {
            match local_packet.packet_type {
                PacketType::NodeInfo => {
                    // Update node info in neighbor table
                    if let Some(info) = self.parse_node_info(&local_packet.payload) {
                        self.neighbors.update_info(local_packet.header.source, info);
                    }
                }
                PacketType::Telemetry => {
                    // Parse and store telemetry from neighbor
                    if let Some(telemetry) = self.parse_telemetry(&local_packet.payload) {
                        self.neighbors.update_telemetry(local_packet.header.source, telemetry);
                    }
                }
                PacketType::Ack => {
                    self.stats.acks_received += 1;
                }
                _ => {}
            }
        }

        // Return packets for application layer
        local.into_iter().collect()
    }

    fn stats(&self) -> MeshStats {
        let mut stats = self.stats.clone();
        stats.channel_utilization = self.mac.channel_utilization_cached();
        stats.neighbor_count = self.neighbors.len();
        stats.route_count = self.next_hop_router.route_count();
        stats
    }

    fn tick(&mut self, _elapsed: Duration) {
        // Prune stale neighbors
        self.neighbors.prune_stale();

        // Prune expired routes
        self.next_hop_router.prune();

        // Check for periodic broadcasts
        if self.config.position_enabled {
            let should_broadcast = self.last_position_broadcast
                .map(|t| t.elapsed() > Duration::from_secs(self.config.position_interval))
                .unwrap_or(true);

            if should_broadcast {
                self.broadcast_position();
            }
        }

        // Check for telemetry broadcast
        if self.config.telemetry.device_update_interval > 0 {
            let interval = Duration::from_secs(self.config.telemetry.device_update_interval as u64);
            let should_broadcast = self.last_telemetry_broadcast
                .map(|t| t.elapsed() > interval)
                .unwrap_or(true);

            if should_broadcast {
                self.broadcast_telemetry();
            }
        }
    }
}

impl MeshtasticNode {
    /// Parse node info from payload
    fn parse_node_info(&self, payload: &[u8]) -> Option<NodeInfo> {
        if payload.len() < 2 {
            return None;
        }

        let short_len = payload[0] as usize;
        if payload.len() < 2 + short_len {
            return None;
        }

        let short_name = String::from_utf8_lossy(&payload[1..1 + short_len]).to_string();

        let long_start = 1 + short_len;
        if payload.len() <= long_start {
            return Some(NodeInfo::with_names(NodeId::UNKNOWN, &short_name, ""));
        }

        let long_len = payload[long_start] as usize;
        let long_name = if payload.len() >= long_start + 1 + long_len {
            String::from_utf8_lossy(&payload[long_start + 1..long_start + 1 + long_len]).to_string()
        } else {
            String::new()
        };

        Some(NodeInfo::with_names(NodeId::UNKNOWN, &short_name, &long_name))
    }

    /// Parse telemetry from payload
    fn parse_telemetry(&self, payload: &[u8]) -> Option<Telemetry> {
        Telemetry::from_bytes(payload)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_modem_presets() {
        let (sf, bw, _cr) = ModemPreset::LongFast.lora_params();
        assert_eq!(sf, 11);
        assert_eq!(bw, 250_000);

        let (sf, bw, _cr) = ModemPreset::LongSlow.lora_params();
        assert_eq!(sf, 12);
        assert_eq!(bw, 125_000);
    }

    #[test]
    fn test_region_frequencies() {
        let (start, end) = Region::US.frequency_range();
        assert!(start < end);
        assert!(start >= 902_000_000);

        let freq = Region::EU.primary_frequency();
        assert!(freq >= 863_000_000 && freq <= 870_000_000);
    }

    #[test]
    fn test_meshtastic_node_creation() {
        let node = MeshtasticNode::with_defaults();
        assert!(!node.node_id().is_unknown());
    }

    #[test]
    fn test_meshtastic_config() {
        let config = MeshtasticConfig {
            short_name: "TEST".to_string(),
            long_name: "Test Node".to_string(),
            region: Region::EU,
            hop_limit: 5,
            ..Default::default()
        };

        let node = MeshtasticNode::new(config);
        assert_eq!(node.config().hop_limit, 5);
        assert_eq!(node.config().region, Region::EU);
    }

    #[test]
    fn test_packet_receive() {
        let mut node = MeshtasticNode::with_defaults();
        let source = NodeId::random();

        let packet = MeshPacket::broadcast(source, b"Hello", 3);
        let delivered = node.on_receive(packet, -80.0, 10.0);

        // Should be delivered (it's a broadcast to all)
        assert_eq!(delivered.len(), 1);

        // Source should now be a neighbor
        assert!(node.neighbors.get(&source).is_some());
    }

    #[test]
    fn test_duplicate_detection() {
        let mut node = MeshtasticNode::with_defaults();
        let source = NodeId::random();

        let packet = MeshPacket::broadcast(source, b"Hello", 3);

        // First receive
        let delivered = node.on_receive(packet.clone(), -80.0, 10.0);
        assert_eq!(delivered.len(), 1);

        // Duplicate should not be delivered
        let delivered = node.on_receive(packet, -80.0, 10.0);
        assert_eq!(delivered.len(), 0);
    }

    #[test]
    fn test_send_message() {
        let mut node = MeshtasticNode::with_defaults();

        // Send broadcast
        let result = node.broadcast(b"Hello mesh!", 3);
        assert!(result.is_ok());

        // Check queue
        assert_eq!(node.mac.queue_depth(), 1);
    }

    #[test]
    fn test_stats() {
        let mut node = MeshtasticNode::with_defaults();
        let source = NodeId::random();

        let packet = MeshPacket::broadcast(source, b"Test", 3);
        node.on_receive(packet, -80.0, 10.0);

        let stats = node.stats();
        assert_eq!(stats.packets_rx, 1);
    }
}
