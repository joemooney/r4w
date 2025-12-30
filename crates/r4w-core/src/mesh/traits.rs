//! Core mesh networking traits
//!
//! This module defines the fundamental traits that mesh networking implementations
//! must satisfy. These traits are designed to be protocol-agnostic, allowing
//! implementations for Meshtastic, custom protocols, or future standards.

use super::packet::{MeshPacket, NodeId};
use super::neighbor::Neighbor;
use super::routing::Route;
use crate::waveform::Waveform;
use std::fmt::Debug;
use std::time::Duration;

/// Errors that can occur in mesh networking operations
#[derive(Debug, Clone, PartialEq)]
pub enum MeshError {
    /// Channel is busy, cannot transmit
    ChannelBusy,
    /// No route to destination
    NoRoute(NodeId),
    /// Packet TTL/hop limit exceeded
    HopLimitExceeded,
    /// Duplicate packet detected
    DuplicatePacket,
    /// Encryption/decryption failed
    CryptoError(String),
    /// Invalid packet format
    InvalidPacket(String),
    /// Physical layer error
    PhyError(String),
    /// Node not found
    NodeNotFound(NodeId),
    /// Queue full, cannot enqueue packet
    QueueFull,
    /// Timeout waiting for acknowledgment
    AckTimeout,
    /// Generic error
    Other(String),
}

impl std::fmt::Display for MeshError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MeshError::ChannelBusy => write!(f, "Channel is busy"),
            MeshError::NoRoute(id) => write!(f, "No route to node {:?}", id),
            MeshError::HopLimitExceeded => write!(f, "Hop limit exceeded"),
            MeshError::DuplicatePacket => write!(f, "Duplicate packet detected"),
            MeshError::CryptoError(s) => write!(f, "Crypto error: {}", s),
            MeshError::InvalidPacket(s) => write!(f, "Invalid packet: {}", s),
            MeshError::PhyError(s) => write!(f, "PHY error: {}", s),
            MeshError::NodeNotFound(id) => write!(f, "Node not found: {:?}", id),
            MeshError::QueueFull => write!(f, "Queue full"),
            MeshError::AckTimeout => write!(f, "Acknowledgment timeout"),
            MeshError::Other(s) => write!(f, "{}", s),
        }
    }
}

impl std::error::Error for MeshError {}

/// Result type for mesh operations
pub type MeshResult<T> = Result<T, MeshError>;

/// The main trait for mesh networking implementations
///
/// This trait defines the core operations that any mesh network must support:
/// neighbor discovery, routing, and packet forwarding. Implementations can
/// choose their own routing algorithms and neighbor management strategies.
///
/// # Type Parameters
///
/// The trait is generic over packet and neighbor types to allow protocol-specific
/// extensions while maintaining a common interface.
pub trait MeshNetwork: Send + Sync + Debug {
    /// Get this node's unique identifier
    fn node_id(&self) -> NodeId;

    /// Discover neighboring nodes
    ///
    /// This initiates the neighbor discovery process, which may involve
    /// sending probe packets or listening for announcements. The actual
    /// discovery may be asynchronous; this returns currently known neighbors.
    fn discover_neighbors(&mut self) -> Vec<Neighbor>;

    /// Get the current neighbor table
    fn neighbors(&self) -> &[Neighbor];

    /// Get a route to the specified destination
    ///
    /// Returns `None` if no route is known. For flood-routed networks,
    /// this may return a generic "broadcast" route.
    fn route(&self, dest: NodeId) -> Option<Route>;

    /// Forward a packet through the mesh
    ///
    /// This is the main entry point for sending packets. The implementation
    /// decides whether to use flood routing, next-hop routing, or a hybrid
    /// approach based on the packet type and destination.
    fn forward(&mut self, packet: MeshPacket) -> MeshResult<()>;

    /// Handle a received packet
    ///
    /// Called when a packet is received from the physical layer. The
    /// implementation should:
    /// 1. Check for duplicates
    /// 2. Update neighbor/routing tables based on the packet
    /// 3. Decide whether to process locally, forward, or both
    /// 4. Return any packets that should be delivered to the application layer
    fn on_receive(&mut self, packet: MeshPacket, rssi: f32, snr: f32) -> Vec<MeshPacket>;

    /// Send a broadcast message
    ///
    /// Broadcasts are delivered to all reachable nodes. The hop_limit
    /// controls how far the message propagates.
    fn broadcast(&mut self, payload: &[u8], hop_limit: u8) -> MeshResult<()> {
        let packet = MeshPacket::broadcast(self.node_id(), payload, hop_limit);
        self.forward(packet)
    }

    /// Send a direct message to a specific node
    ///
    /// Direct messages may be routed through intermediate nodes, but are
    /// only delivered to the specified destination.
    fn send_direct(&mut self, dest: NodeId, payload: &[u8]) -> MeshResult<()> {
        let packet = MeshPacket::direct(self.node_id(), dest, payload);
        self.forward(packet)
    }

    /// Get mesh network statistics
    fn stats(&self) -> MeshStats;

    /// Process pending operations (call periodically)
    ///
    /// This handles timeouts, retransmissions, and other periodic tasks.
    /// Should be called regularly (e.g., every 100ms).
    fn tick(&mut self, elapsed: Duration);
}

/// Extension trait for mesh-capable physical layer waveforms
///
/// This trait extends the base `Waveform` trait with capabilities needed
/// for mesh networking: channel sensing, signal quality measurement,
/// and packet-based transmission.
pub trait MeshPhy: Waveform {
    /// Check if the channel is currently busy
    ///
    /// This is used for CSMA/CA. Returns `true` if transmission should
    /// be deferred.
    fn channel_busy(&self) -> bool;

    /// Get the received signal strength indicator (RSSI) in dBm
    ///
    /// Returns the RSSI of the most recently received signal.
    fn rssi(&self) -> f32;

    /// Get the signal-to-noise ratio (SNR) in dB
    ///
    /// Returns the SNR of the most recently received signal.
    fn snr(&self) -> f32;

    /// Transmit a packet
    ///
    /// Encodes and transmits the packet. This should handle preamble,
    /// sync word, and CRC as appropriate for the protocol.
    fn transmit(&mut self, packet: &[u8]) -> MeshResult<()>;

    /// Receive a packet
    ///
    /// Attempts to receive a packet. Returns `None` if no valid packet
    /// is available.
    fn receive(&mut self) -> Option<Vec<u8>>;

    /// Start channel activity detection (CAD)
    ///
    /// For LoRa, this initiates CAD mode. Returns `true` if a preamble
    /// is detected.
    fn start_cad(&mut self) -> bool {
        self.channel_busy()
    }

    /// Get the current channel frequency in Hz
    fn frequency(&self) -> u64;

    /// Set the channel frequency in Hz
    fn set_frequency(&mut self, freq_hz: u64) -> MeshResult<()>;

    /// Get the current transmit power in dBm
    fn tx_power(&self) -> i8;

    /// Set the transmit power in dBm
    fn set_tx_power(&mut self, power_dbm: i8) -> MeshResult<()>;
}

/// Statistics for mesh network operation
#[derive(Debug, Clone, Default)]
pub struct MeshStats {
    /// Number of packets transmitted
    pub packets_tx: u64,
    /// Number of packets received
    pub packets_rx: u64,
    /// Number of packets forwarded (relayed)
    pub packets_forwarded: u64,
    /// Number of duplicate packets detected
    pub duplicates_dropped: u64,
    /// Number of packets dropped due to hop limit
    pub hop_limit_exceeded: u64,
    /// Number of packets dropped due to queue full
    pub queue_drops: u64,
    /// Number of acknowledgments sent
    pub acks_sent: u64,
    /// Number of acknowledgments received
    pub acks_received: u64,
    /// Number of retransmissions
    pub retransmissions: u64,
    /// Total bytes transmitted
    pub bytes_tx: u64,
    /// Total bytes received
    pub bytes_rx: u64,
    /// Current channel utilization (0.0 - 1.0)
    pub channel_utilization: f32,
    /// Average round-trip time in milliseconds
    pub avg_rtt_ms: f32,
    /// Number of known neighbors
    pub neighbor_count: usize,
    /// Number of known routes
    pub route_count: usize,
}

/// Configuration for mesh network behavior
#[derive(Debug, Clone)]
pub struct MeshConfig {
    /// This node's ID (random if None)
    pub node_id: Option<NodeId>,
    /// Default hop limit for broadcasts
    pub default_hop_limit: u8,
    /// Maximum packet size in bytes
    pub max_packet_size: usize,
    /// How long to keep packets in duplicate cache (seconds)
    pub duplicate_cache_ttl: u64,
    /// Maximum duplicate cache size
    pub duplicate_cache_size: usize,
    /// How long before a neighbor is considered stale (seconds)
    pub neighbor_timeout: u64,
    /// Enable encryption
    pub encryption_enabled: bool,
    /// Pre-shared key for encryption (32 bytes)
    pub psk: Option<[u8; 32]>,
    /// Enable position sharing
    pub position_enabled: bool,
    /// Position update interval in seconds
    pub position_interval: u64,
    /// Enable acknowledgments for direct messages
    pub ack_enabled: bool,
    /// Acknowledgment timeout in milliseconds
    pub ack_timeout_ms: u64,
    /// Maximum retransmission attempts
    pub max_retries: u8,
}

impl Default for MeshConfig {
    fn default() -> Self {
        Self {
            node_id: None,
            default_hop_limit: 3,
            max_packet_size: 256,
            duplicate_cache_ttl: 300, // 5 minutes
            duplicate_cache_size: 256,
            neighbor_timeout: 7200, // 2 hours
            encryption_enabled: false,
            psk: None,
            position_enabled: false,
            position_interval: 900, // 15 minutes
            ack_enabled: true,
            ack_timeout_ms: 5000,
            max_retries: 3,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mesh_error_display() {
        let err = MeshError::NoRoute(NodeId::from_bytes([1, 2, 3, 4]));
        assert!(err.to_string().contains("No route"));
    }

    #[test]
    fn test_mesh_config_default() {
        let config = MeshConfig::default();
        assert_eq!(config.default_hop_limit, 3);
        assert_eq!(config.duplicate_cache_size, 256);
    }
}
