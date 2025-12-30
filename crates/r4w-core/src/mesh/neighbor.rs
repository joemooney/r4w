//! Neighbor discovery and management
//!
//! This module handles tracking of neighboring mesh nodes. It maintains
//! information about nodes that can be directly reached (single hop) and
//! provides the foundation for routing decisions.

use super::packet::NodeId;
use super::telemetry::Telemetry;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Information about a node in the mesh network
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeInfo {
    /// Node's unique identifier
    pub node_id: NodeId,
    /// Short name (up to 4 characters)
    pub short_name: String,
    /// Long name/description (up to 40 characters)
    pub long_name: String,
    /// Hardware model identifier
    pub hardware_model: u8,
    /// Firmware version string
    pub firmware_version: String,
    /// Last known position (latitude, longitude, altitude)
    pub position: Option<(f64, f64, f32)>,
    /// Battery level (0-100, or None if unknown)
    pub battery_level: Option<u8>,
    /// Whether this node is a router
    pub is_router: bool,
}

impl NodeInfo {
    /// Create minimal node info with just an ID
    pub fn new(node_id: NodeId) -> Self {
        Self {
            node_id,
            short_name: String::new(),
            long_name: String::new(),
            hardware_model: 0,
            firmware_version: String::new(),
            position: None,
            battery_level: None,
            is_router: false,
        }
    }

    /// Create node info with names
    pub fn with_names(node_id: NodeId, short_name: &str, long_name: &str) -> Self {
        Self {
            node_id,
            short_name: short_name.chars().take(4).collect(),
            long_name: long_name.chars().take(40).collect(),
            ..Self::new(node_id)
        }
    }
}

/// Link quality metrics for a neighbor
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct LinkQuality {
    /// Received signal strength indicator (dBm)
    pub rssi: f32,
    /// Signal-to-noise ratio (dB)
    pub snr: f32,
    /// Packet delivery ratio (0.0 - 1.0)
    pub pdr: f32,
    /// Average round-trip time (milliseconds)
    pub avg_rtt_ms: Option<f32>,
    /// Number of packets received from this neighbor
    pub packets_received: u32,
    /// Number of packets sent to this neighbor
    pub packets_sent: u32,
}

impl Default for LinkQuality {
    fn default() -> Self {
        Self {
            rssi: -120.0,
            snr: -20.0,
            pdr: 0.0,
            avg_rtt_ms: None,
            packets_received: 0,
            packets_sent: 0,
        }
    }
}

impl LinkQuality {
    /// Create new link quality from first packet
    pub fn new(rssi: f32, snr: f32) -> Self {
        Self {
            rssi,
            snr,
            pdr: 1.0,
            avg_rtt_ms: None,
            packets_received: 1,
            packets_sent: 0,
        }
    }

    /// Update with new measurement using exponential moving average
    pub fn update(&mut self, rssi: f32, snr: f32) {
        const ALPHA: f32 = 0.3; // Weight for new measurements
        self.rssi = ALPHA * rssi + (1.0 - ALPHA) * self.rssi;
        self.snr = ALPHA * snr + (1.0 - ALPHA) * self.snr;
        self.packets_received += 1;
    }

    /// Calculate a quality score (higher is better)
    /// Used for routing decisions
    pub fn quality_score(&self) -> f32 {
        // Normalize RSSI to 0-1 range (-120 dBm to -40 dBm)
        let rssi_norm = ((self.rssi + 120.0) / 80.0).clamp(0.0, 1.0);
        // Normalize SNR to 0-1 range (-20 dB to +30 dB)
        let snr_norm = ((self.snr + 20.0) / 50.0).clamp(0.0, 1.0);
        // Combine with PDR
        rssi_norm * 0.3 + snr_norm * 0.4 + self.pdr * 0.3
    }
}

/// A neighboring node (directly reachable)
#[derive(Debug, Clone)]
pub struct Neighbor {
    /// Node information
    pub info: NodeInfo,
    /// Link quality metrics
    pub link_quality: LinkQuality,
    /// Time of last communication
    last_seen: Instant,
    /// Hop count (always 1 for direct neighbors)
    pub hop_count: u8,
    /// Last received telemetry
    pub telemetry: Option<Telemetry>,
}

impl Neighbor {
    /// Create a new neighbor entry
    pub fn new(node_id: NodeId, rssi: f32, snr: f32) -> Self {
        Self {
            info: NodeInfo::new(node_id),
            link_quality: LinkQuality::new(rssi, snr),
            last_seen: Instant::now(),
            hop_count: 1,
            telemetry: None,
        }
    }

    /// Update neighbor with new packet reception
    pub fn update(&mut self, rssi: f32, snr: f32) {
        self.link_quality.update(rssi, snr);
        self.last_seen = Instant::now();
    }

    /// Update node info
    pub fn set_info(&mut self, info: NodeInfo) {
        self.info = info;
    }

    /// Update telemetry
    pub fn set_telemetry(&mut self, telemetry: Telemetry) {
        self.telemetry = Some(telemetry);
    }

    /// Get time since last seen
    pub fn time_since_seen(&self) -> Duration {
        self.last_seen.elapsed()
    }

    /// Check if neighbor is stale (not seen recently)
    pub fn is_stale(&self, timeout: Duration) -> bool {
        self.last_seen.elapsed() > timeout
    }

    /// Get the node ID
    pub fn node_id(&self) -> NodeId {
        self.info.node_id
    }
}

/// Neighbor table for tracking known nodes
#[derive(Debug)]
pub struct NeighborTable {
    /// Direct neighbors (single hop)
    neighbors: HashMap<NodeId, Neighbor>,
    /// Timeout for considering a neighbor stale
    timeout: Duration,
    /// Maximum number of neighbors to track
    max_entries: usize,
}

impl NeighborTable {
    /// Create a new neighbor table
    pub fn new(timeout_secs: u64, max_entries: usize) -> Self {
        Self {
            neighbors: HashMap::new(),
            timeout: Duration::from_secs(timeout_secs),
            max_entries,
        }
    }

    /// Add or update a neighbor
    pub fn update(&mut self, node_id: NodeId, rssi: f32, snr: f32) {
        if let Some(neighbor) = self.neighbors.get_mut(&node_id) {
            neighbor.update(rssi, snr);
        } else {
            // Check if we need to make room
            if self.neighbors.len() >= self.max_entries {
                self.evict_oldest();
            }
            self.neighbors.insert(node_id, Neighbor::new(node_id, rssi, snr));
        }
    }

    /// Update node info for a neighbor
    pub fn update_info(&mut self, node_id: NodeId, info: NodeInfo) {
        if let Some(neighbor) = self.neighbors.get_mut(&node_id) {
            neighbor.set_info(info);
        }
    }

    /// Update telemetry for a neighbor
    pub fn update_telemetry(&mut self, node_id: NodeId, telemetry: Telemetry) {
        if let Some(neighbor) = self.neighbors.get_mut(&node_id) {
            neighbor.set_telemetry(telemetry);
        }
    }

    /// Get a neighbor by ID
    pub fn get(&self, node_id: &NodeId) -> Option<&Neighbor> {
        self.neighbors.get(node_id)
    }

    /// Get all neighbors
    pub fn all(&self) -> Vec<&Neighbor> {
        self.neighbors.values().collect()
    }

    /// Get all non-stale neighbors
    pub fn active(&self) -> Vec<&Neighbor> {
        self.neighbors
            .values()
            .filter(|n| !n.is_stale(self.timeout))
            .collect()
    }

    /// Remove stale neighbors
    pub fn prune_stale(&mut self) -> usize {
        let timeout = self.timeout;
        let before = self.neighbors.len();
        self.neighbors.retain(|_, n| !n.is_stale(timeout));
        before - self.neighbors.len()
    }

    /// Get the best neighbor for routing (highest quality score)
    pub fn best_neighbor(&self) -> Option<&Neighbor> {
        self.active()
            .into_iter()
            .max_by(|a, b| {
                a.link_quality
                    .quality_score()
                    .partial_cmp(&b.link_quality.quality_score())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    }

    /// Get neighbors sorted by quality (best first)
    pub fn sorted_by_quality(&self) -> Vec<&Neighbor> {
        let mut neighbors: Vec<_> = self.active();
        neighbors.sort_by(|a, b| {
            b.link_quality
                .quality_score()
                .partial_cmp(&a.link_quality.quality_score())
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        neighbors
    }

    /// Get neighbor count
    pub fn len(&self) -> usize {
        self.neighbors.len()
    }

    /// Check if empty
    pub fn is_empty(&self) -> bool {
        self.neighbors.is_empty()
    }

    /// Remove the oldest (least recently seen) neighbor
    fn evict_oldest(&mut self) {
        if let Some(oldest_id) = self
            .neighbors
            .iter()
            .max_by_key(|(_, n)| n.time_since_seen())
            .map(|(id, _)| *id)
        {
            self.neighbors.remove(&oldest_id);
        }
    }

    /// Clear all neighbors
    pub fn clear(&mut self) {
        self.neighbors.clear();
    }
}

impl Default for NeighborTable {
    fn default() -> Self {
        Self::new(7200, 256) // 2 hour timeout, 256 max entries
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_node_info() {
        let id = NodeId::random();
        let info = NodeInfo::with_names(id, "TEST", "Test Node Long Name");
        assert_eq!(info.short_name, "TEST");
        assert_eq!(info.long_name, "Test Node Long Name");
    }

    #[test]
    fn test_link_quality() {
        let mut lq = LinkQuality::new(-80.0, 10.0);
        assert_eq!(lq.packets_received, 1);

        lq.update(-70.0, 15.0);
        assert_eq!(lq.packets_received, 2);
        // RSSI should move toward -70
        assert!(lq.rssi > -80.0);

        let score = lq.quality_score();
        assert!(score > 0.0 && score <= 1.0);
    }

    #[test]
    fn test_neighbor() {
        let id = NodeId::random();
        let neighbor = Neighbor::new(id, -80.0, 10.0);

        assert_eq!(neighbor.hop_count, 1);
        assert!(!neighbor.is_stale(Duration::from_secs(60)));
    }

    #[test]
    fn test_neighbor_table() {
        let mut table = NeighborTable::new(3600, 10);

        let id1 = NodeId::from_bytes([1, 2, 3, 4]);
        let id2 = NodeId::from_bytes([5, 6, 7, 8]);

        table.update(id1, -80.0, 10.0);
        table.update(id2, -70.0, 15.0);

        assert_eq!(table.len(), 2);
        assert!(table.get(&id1).is_some());

        // id2 should be best neighbor (better signal)
        let best = table.best_neighbor().unwrap();
        assert_eq!(best.node_id(), id2);
    }

    #[test]
    fn test_neighbor_table_eviction() {
        let mut table = NeighborTable::new(3600, 2);

        let id1 = NodeId::from_bytes([1, 0, 0, 0]);
        let id2 = NodeId::from_bytes([2, 0, 0, 0]);
        let id3 = NodeId::from_bytes([3, 0, 0, 0]);

        table.update(id1, -80.0, 10.0);
        table.update(id2, -80.0, 10.0);
        assert_eq!(table.len(), 2);

        // Adding third should evict oldest
        table.update(id3, -80.0, 10.0);
        assert_eq!(table.len(), 2);
        assert!(table.get(&id3).is_some());
    }
}
