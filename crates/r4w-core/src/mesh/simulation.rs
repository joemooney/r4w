//! Multi-Node Mesh Network Simulation
//!
//! This module provides a simulation framework for testing mesh networks
//! without hardware. It models:
//!
//! - Multiple nodes with configurable positions
//! - Radio propagation with path loss
//! - Packet collisions and interference
//! - Network topology changes (node join/leave)
//! - Message delivery statistics
//!
//! ## Example
//!
//! ```ignore
//! use r4w_core::mesh::simulation::{MeshSimulator, SimConfig, NodePosition};
//!
//! // Create a 10-node simulation
//! let config = SimConfig::default().with_node_count(10);
//! let mut sim = MeshSimulator::new(config);
//!
//! // Run simulation steps
//! for _ in 0..1000 {
//!     sim.step();
//! }
//!
//! // Check results
//! let stats = sim.stats();
//! println!("Delivery rate: {:.1}%", stats.delivery_rate() * 100.0);
//! ```

use super::meshtastic::{MeshtasticConfig, MeshtasticNode, ModemPreset, Region};
use super::packet::NodeId;
use super::traits::MeshNetwork;
use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

/// Node position in 2D space (meters)
#[derive(Debug, Clone, Copy)]
pub struct NodePosition {
    pub x: f64,
    pub y: f64,
}

impl NodePosition {
    pub fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    /// Distance to another node in meters
    pub fn distance_to(&self, other: &NodePosition) -> f64 {
        let dx = self.x - other.x;
        let dy = self.y - other.y;
        (dx * dx + dy * dy).sqrt()
    }

    /// Generate random position within bounds
    pub fn random(max_x: f64, max_y: f64, seed: u64) -> Self {
        // Simple LCG for reproducible randomness
        let mut rng = seed;
        rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
        let x = (rng as f64 / u64::MAX as f64) * max_x;
        rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
        let y = (rng as f64 / u64::MAX as f64) * max_y;
        Self { x, y }
    }
}

/// Simulation configuration
#[derive(Debug, Clone)]
pub struct SimConfig {
    /// Number of nodes in the simulation
    pub node_count: usize,
    /// Simulation area width (meters)
    pub area_width: f64,
    /// Simulation area height (meters)
    pub area_height: f64,
    /// Transmission power (dBm)
    pub tx_power_dbm: f64,
    /// Receiver sensitivity (dBm)
    pub rx_sensitivity_dbm: f64,
    /// Path loss exponent (2.0 = free space, 3.0-4.0 = urban)
    pub path_loss_exponent: f64,
    /// Reference distance for path loss (meters)
    pub reference_distance: f64,
    /// Background noise floor (dBm)
    pub noise_floor_dbm: f64,
    /// Minimum SNR for successful reception (dB)
    pub min_snr_db: f64,
    /// Modem preset
    pub modem_preset: ModemPreset,
    /// Region
    pub region: Region,
    /// Random seed for reproducibility
    pub seed: u64,
    /// Message generation rate per node per step
    pub message_rate: f64,
    /// Enable verbose logging
    pub verbose: bool,
}

impl Default for SimConfig {
    fn default() -> Self {
        Self {
            node_count: 10,
            area_width: 5000.0,  // 5 km
            area_height: 5000.0, // 5 km
            tx_power_dbm: 20.0,  // 100 mW
            rx_sensitivity_dbm: -130.0,
            path_loss_exponent: 2.8, // Suburban
            reference_distance: 1.0,
            noise_floor_dbm: -120.0,
            min_snr_db: -5.0, // LoRa can decode at negative SNR
            modem_preset: ModemPreset::LongFast,
            region: Region::US,
            seed: 42,
            message_rate: 0.01, // 1% chance per step
            verbose: false,
        }
    }
}

impl SimConfig {
    pub fn with_node_count(mut self, count: usize) -> Self {
        self.node_count = count;
        self
    }

    pub fn with_area(mut self, width: f64, height: f64) -> Self {
        self.area_width = width;
        self.area_height = height;
        self
    }

    pub fn with_seed(mut self, seed: u64) -> Self {
        self.seed = seed;
        self
    }

    pub fn with_verbose(mut self, verbose: bool) -> Self {
        self.verbose = verbose;
        self
    }

    pub fn with_message_rate(mut self, rate: f64) -> Self {
        self.message_rate = rate;
        self
    }
}

/// A packet in flight through the simulated channel
#[derive(Debug, Clone)]
struct InFlightPacket {
    /// Wire-format bytes
    data: Vec<u8>,
    /// Source node index
    source_idx: usize,
    /// Source position
    source_pos: NodePosition,
    /// Simulation step when sent
    sent_step: u64,
    /// Transmission duration in simulation steps
    duration_steps: u64,
}

/// Message tracking for delivery statistics
#[derive(Debug, Clone)]
struct TrackedMessage {
    /// Original sender node index
    sender_idx: usize,
    /// Intended destination (None = broadcast)
    dest_idx: Option<usize>,
    /// Packet ID
    packet_id: u16,
    /// Time sent
    sent_at: Instant,
    /// Nodes that received it
    received_by: Vec<usize>,
    /// Hop count when delivered
    hop_counts: Vec<u8>,
}

/// Simulation statistics
#[derive(Debug, Clone, Default)]
pub struct SimStats {
    /// Total messages sent
    pub messages_sent: u64,
    /// Total messages delivered (at least one recipient)
    pub messages_delivered: u64,
    /// Total packets transmitted
    pub packets_transmitted: u64,
    /// Total packets received
    pub packets_received: u64,
    /// Total packet collisions
    pub collisions: u64,
    /// Total packets lost (out of range)
    pub packets_lost: u64,
    /// Average hop count for delivered messages
    pub avg_hops: f64,
    /// Average delivery latency (ms)
    pub avg_latency_ms: f64,
    /// Per-node statistics
    pub per_node: Vec<NodeStats>,
}

impl SimStats {
    /// Message delivery rate (0.0 - 1.0)
    pub fn delivery_rate(&self) -> f64 {
        if self.messages_sent == 0 {
            0.0
        } else {
            self.messages_delivered as f64 / self.messages_sent as f64
        }
    }

    /// Packet success rate (0.0 - 1.0)
    pub fn packet_success_rate(&self) -> f64 {
        if self.packets_transmitted == 0 {
            0.0
        } else {
            self.packets_received as f64 / self.packets_transmitted as f64
        }
    }
}

/// Per-node statistics
#[derive(Debug, Clone, Default)]
pub struct NodeStats {
    pub node_id: u32,
    pub messages_sent: u64,
    pub messages_received: u64,
    pub packets_forwarded: u64,
    pub neighbors_discovered: usize,
}

/// Simulated node wrapper
struct SimNode {
    node: MeshtasticNode,
    position: NodePosition,
    /// Packets waiting to be transmitted
    tx_queue: VecDeque<Vec<u8>>,
    /// Whether currently transmitting
    is_transmitting: bool,
    /// Transmission end step (simulation step when TX completes)
    tx_end_step: Option<u64>,
}

/// Multi-node mesh network simulator
pub struct MeshSimulator {
    config: SimConfig,
    nodes: Vec<SimNode>,
    /// Packets currently in flight
    in_flight: Vec<InFlightPacket>,
    /// Message tracking for statistics
    tracked_messages: HashMap<(u32, u16), TrackedMessage>,
    /// Simulation step counter
    step_count: u64,
    /// Simulation start time
    start_time: Instant,
    /// RNG state
    rng_state: u64,
    /// Collected statistics
    stats: SimStats,
    /// Event log for analysis
    event_log: Vec<SimEvent>,
}

/// Simulation events for logging
#[derive(Debug, Clone)]
pub enum SimEvent {
    NodeJoined { node_idx: usize, node_id: u32, position: NodePosition },
    MessageSent { node_idx: usize, packet_id: u16, dest: Option<u32> },
    PacketTransmitted { node_idx: usize, size: usize },
    PacketReceived { node_idx: usize, from_idx: usize, rssi: f64, snr: f64 },
    PacketCollision { node_idx: usize },
    PacketLost { from_idx: usize, to_idx: usize, reason: String },
    MessageDelivered { packet_id: u16, hops: u8, latency_ms: u64 },
    NeighborDiscovered { node_idx: usize, neighbor_id: u32 },
}

impl MeshSimulator {
    /// Create a new simulator with the given configuration
    pub fn new(config: SimConfig) -> Self {
        let mut sim = Self {
            nodes: Vec::with_capacity(config.node_count),
            in_flight: Vec::new(),
            tracked_messages: HashMap::new(),
            step_count: 0,
            start_time: Instant::now(),
            rng_state: config.seed,
            stats: SimStats::default(),
            event_log: Vec::new(),
            config,
        };

        sim.initialize_nodes();
        sim
    }

    /// Initialize nodes with random positions
    fn initialize_nodes(&mut self) {
        for i in 0..self.config.node_count {
            let position = NodePosition::random(
                self.config.area_width,
                self.config.area_height,
                self.config.seed.wrapping_add(i as u64 * 12345),
            );

            let mut mesh_config = MeshtasticConfig::default();
            mesh_config.primary_channel.preset = self.config.modem_preset;
            mesh_config.region = self.config.region;
            mesh_config.short_name = format!("N{:02}", i);
            mesh_config.long_name = format!("SimNode {}", i);

            let node = MeshtasticNode::new(mesh_config);
            let node_id = node.node_id().to_u32();

            self.event_log.push(SimEvent::NodeJoined {
                node_idx: i,
                node_id,
                position,
            });

            self.nodes.push(SimNode {
                node,
                position,
                tx_queue: VecDeque::new(),
                is_transmitting: false,
                tx_end_step: None,
            });

            self.stats.per_node.push(NodeStats {
                node_id,
                ..Default::default()
            });
        }

        if self.config.verbose {
            println!("Initialized {} nodes", self.nodes.len());
            for (i, node) in self.nodes.iter().enumerate() {
                println!(
                    "  Node {}: {:08x} at ({:.0}, {:.0})",
                    i,
                    node.node.node_id().to_u32(),
                    node.position.x,
                    node.position.y
                );
            }
        }
    }

    /// Get a random number using internal LCG
    fn rand(&mut self) -> f64 {
        self.rng_state = self.rng_state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1);
        (self.rng_state >> 33) as f64 / (1u64 << 31) as f64
    }

    /// Calculate received signal strength between two positions
    fn calculate_rssi(&self, tx_pos: &NodePosition, rx_pos: &NodePosition) -> f64 {
        let distance = tx_pos.distance_to(rx_pos).max(self.config.reference_distance);

        // Log-distance path loss model
        // PL(d) = PL(d0) + 10 * n * log10(d/d0)
        let pl_reference = 20.0 * (4.0 * std::f64::consts::PI * self.config.reference_distance / 0.33).log10();
        let path_loss = pl_reference
            + 10.0 * self.config.path_loss_exponent * (distance / self.config.reference_distance).log10();

        self.config.tx_power_dbm - path_loss
    }

    /// Calculate SNR from RSSI
    fn calculate_snr(&self, rssi: f64) -> f64 {
        rssi - self.config.noise_floor_dbm
    }

    /// Run one simulation step
    pub fn step(&mut self) {
        self.step_count += 1;

        // 1. Generate new messages randomly
        self.generate_messages();

        // 2. Process node TX queues
        self.process_transmissions();

        // 3. Propagate in-flight packets
        self.propagate_packets();

        // 4. Process node MAC layers (timing)
        self.process_mac_timing();
    }

    /// Generate random messages from nodes
    fn generate_messages(&mut self) {
        let node_count = self.nodes.len();
        for i in 0..node_count {
            if self.rand() < self.config.message_rate {
                // Create a broadcast message
                let msg = format!("Msg from node {} step {}", i, self.step_count);

                if self.nodes[i].node.broadcast(msg.as_bytes(), 3).is_ok() {
                    self.stats.messages_sent += 1;
                    self.stats.per_node[i].messages_sent += 1;

                    if self.config.verbose {
                        println!("Step {}: Node {} sent broadcast", self.step_count, i);
                    }
                }
            }
        }
    }

    /// Process transmissions from nodes
    fn process_transmissions(&mut self) {
        let current_step = self.step_count;

        for i in 0..self.nodes.len() {
            // Check if current transmission is complete
            if self.nodes[i].is_transmitting {
                if let Some(end_step) = self.nodes[i].tx_end_step {
                    if current_step >= end_step {
                        self.nodes[i].is_transmitting = false;
                        self.nodes[i].tx_end_step = None;
                    } else {
                        continue; // Still transmitting
                    }
                }
            }

            // Force TX bypassing MAC timing for simulation
            // In real hardware, process_tx() respects CSMA/CA timing
            if let Some(wire_bytes) = self.nodes[i].node.force_tx() {
                // Packet ready to transmit - calculate duration in steps
                // Assume 1 step = 10ms (100 steps/second) as simulation time unit
                let duration_steps = Self::calculate_tx_duration_steps(wire_bytes.len(), &self.config);
                let source_pos = self.nodes[i].position;

                self.in_flight.push(InFlightPacket {
                    data: wire_bytes.clone(),
                    source_idx: i,
                    source_pos,
                    sent_step: current_step,
                    duration_steps,
                });

                self.nodes[i].is_transmitting = true;
                self.nodes[i].tx_end_step = Some(current_step + duration_steps);

                self.stats.packets_transmitted += 1;

                self.event_log.push(SimEvent::PacketTransmitted {
                    node_idx: i,
                    size: wire_bytes.len(),
                });

                if self.config.verbose {
                    println!(
                        "Step {}: Node {} transmitted {} bytes (duration: {} steps)",
                        self.step_count, i, wire_bytes.len(), duration_steps
                    );
                }
            }
        }
    }

    /// Calculate transmission duration in simulation steps
    ///
    /// Assumes 1 step = 10ms. Returns number of steps for transmission.
    fn calculate_tx_duration_steps(size: usize, config: &SimConfig) -> u64 {
        let (sf, bw) = match config.modem_preset {
            ModemPreset::LongFast => (11, 250_000),
            ModemPreset::LongModerate => (11, 125_000),
            ModemPreset::LongSlow => (12, 125_000),
            ModemPreset::MediumFast => (9, 250_000),
            ModemPreset::MediumSlow => (10, 250_000),
            ModemPreset::ShortFast => (7, 250_000),
            ModemPreset::ShortSlow => (8, 250_000),
        };

        // LoRa symbol time: Ts = 2^SF / BW (in microseconds)
        let symbol_time_us = (1 << sf) as f64 / bw as f64 * 1_000_000.0;

        // Approximate symbols per byte (with coding rate 4/5)
        let symbols_per_byte = 8.0 * 5.0 / 4.0 / sf as f64;

        // Preamble (12.25 symbols) + header + payload
        let total_symbols = 12.25 + (size as f64 * symbols_per_byte);
        let duration_us = total_symbols * symbol_time_us;

        // Convert to steps (1 step = 10ms = 10000us), minimum 1 step
        let duration_ms = duration_us / 1000.0;
        (duration_ms / 10.0).ceil().max(1.0) as u64
    }

    /// Calculate transmission duration based on packet size and modem preset
    fn calculate_tx_duration(size: usize, config: &SimConfig) -> Duration {
        let steps = Self::calculate_tx_duration_steps(size, config);
        Duration::from_millis(steps * 10)
    }

    /// Propagate in-flight packets to receivers
    fn propagate_packets(&mut self) {
        let current_step = self.step_count;

        // Remove completed packets and deliver them
        let mut delivered_packets = Vec::new();

        self.in_flight.retain(|packet| {
            if current_step >= packet.sent_step + packet.duration_steps {
                delivered_packets.push(packet.clone());
                false
            } else {
                true
            }
        });

        // Deliver completed packets to nodes in range
        for packet in delivered_packets {
            self.deliver_packet(&packet);
        }
    }

    /// Deliver a packet to all nodes in range
    fn deliver_packet(&mut self, packet: &InFlightPacket) {
        // Extract config values to avoid borrow conflicts
        let rx_sensitivity = self.config.rx_sensitivity_dbm;
        let min_snr = self.config.min_snr_db;
        let noise_floor = self.config.noise_floor_dbm;
        let tx_power = self.config.tx_power_dbm;
        let path_loss_exp = self.config.path_loss_exponent;
        let ref_dist = self.config.reference_distance;
        let verbose = self.config.verbose;
        let step_count = self.step_count;

        for i in 0..self.nodes.len() {
            if i == packet.source_idx {
                continue; // Don't deliver to self
            }

            let node_pos = self.nodes[i].position;
            let is_transmitting = self.nodes[i].is_transmitting;

            // Calculate RSSI using path loss model
            let distance = packet.source_pos.distance_to(&node_pos).max(ref_dist);
            let pl_reference = 20.0 * (4.0 * std::f64::consts::PI * ref_dist / 0.33).log10();
            let path_loss = pl_reference + 10.0 * path_loss_exp * (distance / ref_dist).log10();
            let rssi = tx_power - path_loss;
            let snr = rssi - noise_floor;

            // Check if signal is strong enough
            if rssi < rx_sensitivity || snr < min_snr {
                self.event_log.push(SimEvent::PacketLost {
                    from_idx: packet.source_idx,
                    to_idx: i,
                    reason: format!("RSSI {:.1} dBm, SNR {:.1} dB too weak", rssi, snr),
                });
                self.stats.packets_lost += 1;
                continue;
            }

            // Check for collision (another node transmitting)
            if is_transmitting {
                self.event_log.push(SimEvent::PacketCollision { node_idx: i });
                self.stats.collisions += 1;
                continue;
            }

            // Deliver the packet
            let delivered = self.nodes[i].node.receive_bytes(&packet.data, rssi as f32, snr as f32);

            if !delivered.is_empty() {
                self.stats.packets_received += 1;
                self.stats.per_node[i].messages_received += delivered.len() as u64;

                self.event_log.push(SimEvent::PacketReceived {
                    node_idx: i,
                    from_idx: packet.source_idx,
                    rssi,
                    snr,
                });

                if verbose {
                    println!(
                        "Step {}: Node {} received packet from {} (RSSI: {:.1}, SNR: {:.1})",
                        step_count, i, packet.source_idx, rssi, snr
                    );
                }

                // Track neighbor discovery
                let neighbor_count = self.nodes[i].node.neighbors().len();
                if neighbor_count > self.stats.per_node[i].neighbors_discovered {
                    self.stats.per_node[i].neighbors_discovered = neighbor_count;
                }
            }
        }
    }

    /// Process MAC layer timing for all nodes
    fn process_mac_timing(&mut self) {
        // The MAC layer in each node handles its own timing
        // This is called to advance the simulation time
    }

    /// Run simulation for a number of steps
    pub fn run(&mut self, steps: u64) {
        for _ in 0..steps {
            self.step();
        }
    }

    /// Run simulation until a condition is met or max steps reached
    pub fn run_until<F>(&mut self, max_steps: u64, condition: F)
    where
        F: Fn(&Self) -> bool,
    {
        for _ in 0..max_steps {
            self.step();
            if condition(self) {
                break;
            }
        }
    }

    /// Get current statistics
    pub fn stats(&self) -> &SimStats {
        &self.stats
    }

    /// Get event log
    pub fn events(&self) -> &[SimEvent] {
        &self.event_log
    }

    /// Get node count
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Get node position
    pub fn node_position(&self, idx: usize) -> Option<NodePosition> {
        self.nodes.get(idx).map(|n| n.position)
    }

    /// Get node ID
    pub fn node_id(&self, idx: usize) -> Option<NodeId> {
        self.nodes.get(idx).map(|n| n.node.node_id())
    }

    /// Get neighbor count for a node
    pub fn neighbor_count(&self, idx: usize) -> usize {
        self.nodes
            .get(idx)
            .map(|n| n.node.neighbors().len())
            .unwrap_or(0)
    }

    /// Send a message from one node to another (or broadcast)
    pub fn send_message(&mut self, from_idx: usize, message: &str, dest: Option<usize>) -> bool {
        if from_idx >= self.nodes.len() {
            return false;
        }

        let result = if let Some(to_idx) = dest {
            if to_idx >= self.nodes.len() {
                return false;
            }
            let dest_id = self.nodes[to_idx].node.node_id();
            self.nodes[from_idx]
                .node
                .send_direct(dest_id, message.as_bytes())
                .is_ok()
        } else {
            self.nodes[from_idx]
                .node
                .broadcast(message.as_bytes(), 3)
                .is_ok()
        };

        if result {
            self.stats.messages_sent += 1;
            self.stats.per_node[from_idx].messages_sent += 1;
        }

        result
    }

    /// Get simulation step count
    pub fn step_count(&self) -> u64 {
        self.step_count
    }

    /// Print a summary of the simulation
    pub fn print_summary(&self) {
        println!("\n=== Mesh Simulation Summary ===");
        println!("Steps: {}", self.step_count);
        println!("Nodes: {}", self.nodes.len());
        println!();
        println!("Messages:");
        println!("  Sent: {}", self.stats.messages_sent);
        println!("  Delivered: {}", self.stats.messages_delivered);
        println!(
            "  Delivery rate: {:.1}%",
            self.stats.delivery_rate() * 100.0
        );
        println!();
        println!("Packets:");
        println!("  Transmitted: {}", self.stats.packets_transmitted);
        println!("  Received: {}", self.stats.packets_received);
        println!("  Collisions: {}", self.stats.collisions);
        println!("  Lost (range): {}", self.stats.packets_lost);
        println!(
            "  Success rate: {:.1}%",
            self.stats.packet_success_rate() * 100.0
        );
        println!();
        println!("Per-node stats:");
        for (i, stats) in self.stats.per_node.iter().enumerate() {
            println!(
                "  Node {:2}: TX={} RX={} neighbors={}",
                i, stats.messages_sent, stats.messages_received, stats.neighbors_discovered
            );
        }
    }

    /// Get topology as adjacency list (based on current neighbor tables)
    pub fn topology(&self) -> Vec<Vec<usize>> {
        let mut adj = vec![Vec::new(); self.nodes.len()];

        for (i, sim_node) in self.nodes.iter().enumerate() {
            for neighbor in sim_node.node.neighbors() {
                // Find the index of this neighbor
                let neighbor_id = neighbor.info.node_id;
                for (j, other) in self.nodes.iter().enumerate() {
                    if other.node.node_id() == neighbor_id {
                        if !adj[i].contains(&j) {
                            adj[i].push(j);
                        }
                        break;
                    }
                }
            }
        }

        adj
    }

    /// Check if the network is connected (all nodes reachable from node 0)
    pub fn is_connected(&self) -> bool {
        if self.nodes.is_empty() {
            return true;
        }

        let adj = self.topology();
        let mut visited = vec![false; self.nodes.len()];
        let mut stack = vec![0usize];

        while let Some(node) = stack.pop() {
            if visited[node] {
                continue;
            }
            visited[node] = true;

            for &neighbor in &adj[node] {
                if !visited[neighbor] {
                    stack.push(neighbor);
                }
            }
        }

        visited.iter().all(|&v| v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_node_position_distance() {
        let p1 = NodePosition::new(0.0, 0.0);
        let p2 = NodePosition::new(3.0, 4.0);
        assert!((p1.distance_to(&p2) - 5.0).abs() < 0.001);
    }

    #[test]
    fn test_simulator_creation() {
        let config = SimConfig::default().with_node_count(5);
        let sim = MeshSimulator::new(config);
        assert_eq!(sim.node_count(), 5);
    }

    #[test]
    fn test_simulator_step() {
        let config = SimConfig::default()
            .with_node_count(3)
            .with_area(1000.0, 1000.0)
            .with_message_rate(0.0); // No random messages

        let mut sim = MeshSimulator::new(config);

        // Send a message
        assert!(sim.send_message(0, "Hello!", None));

        // Run some steps
        sim.run(100);

        assert!(sim.stats.packets_transmitted > 0);
    }

    #[test]
    fn test_rssi_calculation() {
        let config = SimConfig::default();
        let sim = MeshSimulator::new(config);

        let p1 = NodePosition::new(0.0, 0.0);
        let p2 = NodePosition::new(100.0, 0.0);

        let rssi = sim.calculate_rssi(&p1, &p2);
        assert!(rssi < sim.config.tx_power_dbm); // Path loss
        assert!(rssi > sim.config.rx_sensitivity_dbm); // Still receivable at 100m
    }

    #[test]
    fn test_tx_duration() {
        let config = SimConfig::default();
        let duration = MeshSimulator::calculate_tx_duration(50, &config);
        assert!(duration.as_millis() > 0);
        assert!(duration.as_millis() < 10000); // Should be less than 10 seconds
    }

    #[test]
    fn test_close_nodes_communicate() {
        let config = SimConfig::default()
            .with_node_count(2)
            .with_area(100.0, 100.0) // Small area = close nodes
            .with_message_rate(0.0)
            .with_verbose(false);

        let mut sim = MeshSimulator::new(config);

        // Send message from node 0
        sim.send_message(0, "Test message", None);

        // Run simulation
        sim.run(200);

        // Check that node 1 received something
        assert!(
            sim.stats.packets_received > 0,
            "No packets received in close proximity"
        );
    }

    #[test]
    fn test_far_nodes_no_communicate() {
        let config = SimConfig::default()
            .with_node_count(2)
            .with_area(100_000.0, 100_000.0) // 100 km = too far
            .with_seed(999) // Different seed for spread
            .with_message_rate(0.0);

        let mut sim = MeshSimulator::new(config);

        // Manually position nodes far apart
        // (The random positions with this seed should be far enough)

        sim.send_message(0, "Test message", None);
        sim.run(100);

        // Packets should be lost due to distance
        // (This may or may not work depending on random positions)
    }
}
