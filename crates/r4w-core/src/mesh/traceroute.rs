//! Mesh Traceroute
//!
//! Implements traceroute functionality for mesh networks, allowing nodes to
//! discover the path packets take through the mesh and measure hop-by-hop latency.
//!
//! ## Protocol
//!
//! 1. Initiator sends a RouteRequest packet to destination
//! 2. Each hop along the path records its node ID in the packet
//! 3. Destination sends RouteReply back with full path
//! 4. Initiator receives complete route information
//!
//! ## Example
//!
//! ```ignore
//! use r4w_core::mesh::traceroute::{Traceroute, TracerouteConfig};
//!
//! let mut tr = Traceroute::new(my_node_id, TracerouteConfig::default());
//!
//! // Initiate traceroute to a destination
//! let request = tr.start_trace(dest_node_id);
//! // Send request through mesh...
//!
//! // When response arrives
//! let result = tr.handle_response(response_packet);
//! for hop in result.hops {
//!     println!("Hop {}: Node {:08x}, RTT: {:?}", hop.hop_number, hop.node_id, hop.rtt);
//! }
//! ```

use super::packet::NodeId;
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Traceroute configuration
#[derive(Debug, Clone)]
pub struct TracerouteConfig {
    /// Maximum hops to trace
    pub max_hops: u8,
    /// Timeout for waiting for response
    pub timeout: Duration,
    /// Number of probes per hop
    pub probes_per_hop: u8,
    /// Include SNR/RSSI measurements
    pub include_signal_quality: bool,
}

impl Default for TracerouteConfig {
    fn default() -> Self {
        Self {
            max_hops: 10,
            timeout: Duration::from_secs(30),
            probes_per_hop: 1,
            include_signal_quality: true,
        }
    }
}

/// A single hop in the route
#[derive(Debug, Clone)]
pub struct TracerouteHop {
    /// Hop number (1-based)
    pub hop_number: u8,
    /// Node ID at this hop
    pub node_id: NodeId,
    /// Round-trip time to this hop
    pub rtt: Option<Duration>,
    /// Signal quality (RSSI) at this hop
    pub rssi: Option<f32>,
    /// Signal-to-noise ratio at this hop
    pub snr: Option<f32>,
}

/// Result of a traceroute
#[derive(Debug, Clone)]
pub struct TracerouteResult {
    /// Request ID
    pub request_id: u32,
    /// Source node
    pub source: NodeId,
    /// Destination node
    pub destination: NodeId,
    /// Whether destination was reached
    pub reached: bool,
    /// All hops in the path
    pub hops: Vec<TracerouteHop>,
    /// Total round-trip time
    pub total_rtt: Option<Duration>,
    /// When the trace started
    pub started_at: Instant,
    /// When the trace completed
    pub completed_at: Option<Instant>,
}

impl TracerouteResult {
    fn new(request_id: u32, source: NodeId, destination: NodeId) -> Self {
        Self {
            request_id,
            source,
            destination,
            reached: false,
            hops: Vec::new(),
            total_rtt: None,
            started_at: Instant::now(),
            completed_at: None,
        }
    }

    /// Get hop count
    pub fn hop_count(&self) -> usize {
        self.hops.len()
    }

    /// Get average RTT per hop
    pub fn avg_rtt_per_hop(&self) -> Option<Duration> {
        if self.hops.is_empty() {
            return None;
        }

        let sum: Duration = self.hops
            .iter()
            .filter_map(|h| h.rtt)
            .sum();

        let count = self.hops.iter().filter(|h| h.rtt.is_some()).count();
        if count > 0 {
            Some(sum / count as u32)
        } else {
            None
        }
    }

    /// Format as string for display
    pub fn format(&self) -> String {
        let mut output = String::new();
        output.push_str(&format!(
            "Traceroute from {:08x} to {:08x}\n",
            self.source.to_u32(),
            self.destination.to_u32()
        ));
        output.push_str(&format!("Hops: {}\n", self.hop_count()));

        for hop in &self.hops {
            let rtt_str = hop.rtt
                .map(|d| format!("{:.1}ms", d.as_secs_f64() * 1000.0))
                .unwrap_or_else(|| "*".to_string());

            let signal_str = match (hop.rssi, hop.snr) {
                (Some(rssi), Some(snr)) => format!(" [RSSI: {:.0}dBm, SNR: {:.1}dB]", rssi, snr),
                (Some(rssi), None) => format!(" [RSSI: {:.0}dBm]", rssi),
                _ => String::new(),
            };

            output.push_str(&format!(
                "  {:2}. {:08x}  {}{}\n",
                hop.hop_number,
                hop.node_id.to_u32(),
                rtt_str,
                signal_str
            ));
        }

        if self.reached {
            if let Some(total) = self.total_rtt {
                output.push_str(&format!(
                    "Destination reached in {:.1}ms\n",
                    total.as_secs_f64() * 1000.0
                ));
            }
        } else {
            output.push_str("Destination NOT reached\n");
        }

        output
    }
}

/// Route request packet format
#[derive(Debug, Clone)]
pub struct RouteRequest {
    /// Request ID
    pub request_id: u32,
    /// Original source
    pub source: NodeId,
    /// Final destination
    pub destination: NodeId,
    /// Current hop count
    pub hop_count: u8,
    /// Maximum hops allowed
    pub max_hops: u8,
    /// Recorded route (node IDs visited)
    pub route: Vec<NodeId>,
    /// Include signal quality in response
    pub want_signal_quality: bool,
}

impl RouteRequest {
    /// Create a new route request
    pub fn new(
        request_id: u32,
        source: NodeId,
        destination: NodeId,
        max_hops: u8,
    ) -> Self {
        Self {
            request_id,
            source,
            destination,
            hop_count: 0,
            max_hops,
            route: vec![source],
            want_signal_quality: true,
        }
    }

    /// Record a hop
    pub fn add_hop(&mut self, node_id: NodeId) {
        self.hop_count += 1;
        self.route.push(node_id);
    }

    /// Check if max hops reached
    pub fn max_hops_reached(&self) -> bool {
        self.hop_count >= self.max_hops
    }

    /// Serialize to bytes
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(16 + self.route.len() * 4);

        bytes.extend_from_slice(&self.request_id.to_le_bytes());
        bytes.extend_from_slice(&self.source.to_u32().to_le_bytes());
        bytes.extend_from_slice(&self.destination.to_u32().to_le_bytes());
        bytes.push(self.hop_count);
        bytes.push(self.max_hops);
        bytes.push(if self.want_signal_quality { 1 } else { 0 });
        bytes.push(self.route.len() as u8);

        for node_id in &self.route {
            bytes.extend_from_slice(&node_id.to_u32().to_le_bytes());
        }

        bytes
    }

    /// Deserialize from bytes
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 16 {
            return None;
        }

        let request_id = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        let source = NodeId::from_u32(u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]));
        let destination = NodeId::from_u32(u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]));
        let hop_count = bytes[12];
        let max_hops = bytes[13];
        let want_signal_quality = bytes[14] != 0;
        let route_len = bytes[15] as usize;

        if bytes.len() < 16 + route_len * 4 {
            return None;
        }

        let mut route = Vec::with_capacity(route_len);
        for i in 0..route_len {
            let offset = 16 + i * 4;
            let node_id = u32::from_le_bytes([
                bytes[offset],
                bytes[offset + 1],
                bytes[offset + 2],
                bytes[offset + 3],
            ]);
            route.push(NodeId::from_u32(node_id));
        }

        Some(Self {
            request_id,
            source,
            destination,
            hop_count,
            max_hops,
            route,
            want_signal_quality,
        })
    }
}

/// Route reply packet format
#[derive(Debug, Clone)]
pub struct RouteReply {
    /// Request ID (matches the request)
    pub request_id: u32,
    /// Original source (who initiated)
    pub source: NodeId,
    /// Final destination (who replied)
    pub destination: NodeId,
    /// Complete route from source to destination
    pub route: Vec<NodeId>,
    /// Signal quality per hop (RSSI, SNR)
    pub signal_quality: Vec<(f32, f32)>,
    /// Success flag
    pub reached: bool,
}

impl RouteReply {
    /// Create a new route reply from a request
    pub fn from_request(request: &RouteRequest, reached: bool) -> Self {
        Self {
            request_id: request.request_id,
            source: request.source,
            destination: request.destination,
            route: request.route.clone(),
            signal_quality: Vec::new(),
            reached,
        }
    }

    /// Add signal quality for a hop
    pub fn add_signal_quality(&mut self, rssi: f32, snr: f32) {
        self.signal_quality.push((rssi, snr));
    }

    /// Serialize to bytes
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(20 + self.route.len() * 4 + self.signal_quality.len() * 8);

        bytes.extend_from_slice(&self.request_id.to_le_bytes());
        bytes.extend_from_slice(&self.source.to_u32().to_le_bytes());
        bytes.extend_from_slice(&self.destination.to_u32().to_le_bytes());
        bytes.push(if self.reached { 1 } else { 0 });
        bytes.push(self.route.len() as u8);
        bytes.push(self.signal_quality.len() as u8);
        bytes.push(0); // Reserved

        for node_id in &self.route {
            bytes.extend_from_slice(&node_id.to_u32().to_le_bytes());
        }

        for (rssi, snr) in &self.signal_quality {
            bytes.extend_from_slice(&rssi.to_le_bytes());
            bytes.extend_from_slice(&snr.to_le_bytes());
        }

        bytes
    }

    /// Deserialize from bytes
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 16 {
            return None;
        }

        let request_id = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        let source = NodeId::from_u32(u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]));
        let destination = NodeId::from_u32(u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]));
        let reached = bytes[12] != 0;
        let route_len = bytes[13] as usize;
        let signal_len = bytes[14] as usize;
        // bytes[15] is reserved

        let expected_len = 16 + route_len * 4 + signal_len * 8;
        if bytes.len() < expected_len {
            return None;
        }

        let mut route = Vec::with_capacity(route_len);
        for i in 0..route_len {
            let offset = 16 + i * 4;
            let node_id = u32::from_le_bytes([
                bytes[offset],
                bytes[offset + 1],
                bytes[offset + 2],
                bytes[offset + 3],
            ]);
            route.push(NodeId::from_u32(node_id));
        }

        let signal_offset = 16 + route_len * 4;
        let mut signal_quality = Vec::with_capacity(signal_len);
        for i in 0..signal_len {
            let offset = signal_offset + i * 8;
            let rssi = f32::from_le_bytes([
                bytes[offset],
                bytes[offset + 1],
                bytes[offset + 2],
                bytes[offset + 3],
            ]);
            let snr = f32::from_le_bytes([
                bytes[offset + 4],
                bytes[offset + 5],
                bytes[offset + 6],
                bytes[offset + 7],
            ]);
            signal_quality.push((rssi, snr));
        }

        Some(Self {
            request_id,
            source,
            destination,
            route,
            signal_quality,
            reached,
        })
    }
}

/// Traceroute manager
pub struct Traceroute {
    /// Our node ID
    our_node_id: NodeId,
    /// Configuration
    config: TracerouteConfig,
    /// Pending traces (request_id -> result)
    pending: HashMap<u32, TracerouteResult>,
    /// Completed traces
    completed: Vec<TracerouteResult>,
    /// Next request ID
    next_request_id: u32,
}

impl Traceroute {
    /// Create a new Traceroute manager
    pub fn new(our_node_id: NodeId, config: TracerouteConfig) -> Self {
        Self {
            our_node_id,
            config,
            pending: HashMap::new(),
            completed: Vec::new(),
            next_request_id: 1,
        }
    }

    /// Create with default configuration
    pub fn with_defaults(our_node_id: NodeId) -> Self {
        Self::new(our_node_id, TracerouteConfig::default())
    }

    /// Start a traceroute to a destination
    pub fn start_trace(&mut self, destination: NodeId) -> RouteRequest {
        let request_id = self.next_request_id;
        self.next_request_id = self.next_request_id.wrapping_add(1);

        let result = TracerouteResult::new(request_id, self.our_node_id, destination);
        self.pending.insert(request_id, result);

        RouteRequest::new(
            request_id,
            self.our_node_id,
            destination,
            self.config.max_hops,
        )
    }

    /// Handle receiving a route request (as an intermediate hop or destination)
    pub fn handle_request(
        &self,
        mut request: RouteRequest,
        rssi: f32,
        snr: f32,
    ) -> RouteRequestResponse {
        // Add ourselves to the route
        request.add_hop(self.our_node_id);

        // Check if we are the destination
        if self.our_node_id == request.destination {
            let mut reply = RouteReply::from_request(&request, true);
            if request.want_signal_quality {
                reply.add_signal_quality(rssi, snr);
            }
            return RouteRequestResponse::Reply(reply);
        }

        // Check max hops
        if request.max_hops_reached() {
            let reply = RouteReply::from_request(&request, false);
            return RouteRequestResponse::Reply(reply);
        }

        // Forward the request
        RouteRequestResponse::Forward(request)
    }

    /// Handle receiving a route reply
    pub fn handle_reply(&mut self, reply: RouteReply) -> Option<TracerouteResult> {
        if let Some(mut result) = self.pending.remove(&reply.request_id) {
            result.reached = reply.reached;
            result.completed_at = Some(Instant::now());
            result.total_rtt = result.completed_at.map(|t| t - result.started_at);

            // Build hop list from route
            for (i, &node_id) in reply.route.iter().enumerate() {
                let hop = TracerouteHop {
                    hop_number: (i + 1) as u8,
                    node_id,
                    rtt: None, // Can't determine individual RTT from this protocol
                    rssi: reply.signal_quality.get(i).map(|(r, _)| *r),
                    snr: reply.signal_quality.get(i).map(|(_, s)| *s),
                };
                result.hops.push(hop);
            }

            self.completed.push(result.clone());
            Some(result)
        } else {
            None // Unknown request ID
        }
    }

    /// Check for timed out traces
    pub fn check_timeouts(&mut self) -> Vec<TracerouteResult> {
        let timeout = self.config.timeout;
        let mut timed_out = Vec::new();

        self.pending.retain(|_, result| {
            if result.started_at.elapsed() > timeout {
                let mut r = result.clone();
                r.reached = false;
                r.completed_at = Some(Instant::now());
                timed_out.push(r);
                false
            } else {
                true
            }
        });

        for result in &timed_out {
            self.completed.push(result.clone());
        }

        timed_out
    }

    /// Get pending trace count
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    /// Get completed traces
    pub fn completed_traces(&self) -> &[TracerouteResult] {
        &self.completed
    }

    /// Clear completed traces
    pub fn clear_completed(&mut self) {
        self.completed.clear();
    }
}

/// Response type for handling route requests
#[derive(Debug)]
pub enum RouteRequestResponse {
    /// Forward the request to next hop
    Forward(RouteRequest),
    /// Send reply back to source
    Reply(RouteReply),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_node_id(n: u32) -> NodeId {
        NodeId::from_u32(n)
    }

    #[test]
    fn test_route_request_serialization() {
        let request = RouteRequest::new(
            1234,
            test_node_id(1),
            test_node_id(10),
            5,
        );

        let bytes = request.to_bytes();
        let decoded = RouteRequest::from_bytes(&bytes).unwrap();

        assert_eq!(decoded.request_id, 1234);
        assert_eq!(decoded.source.to_u32(), 1);
        assert_eq!(decoded.destination.to_u32(), 10);
        assert_eq!(decoded.max_hops, 5);
        assert_eq!(decoded.route.len(), 1);
    }

    #[test]
    fn test_route_reply_serialization() {
        let request = RouteRequest::new(
            5678,
            test_node_id(1),
            test_node_id(5),
            10,
        );

        let mut reply = RouteReply::from_request(&request, true);
        reply.add_signal_quality(-80.0, 10.5);
        reply.add_signal_quality(-75.0, 12.0);

        let bytes = reply.to_bytes();
        let decoded = RouteReply::from_bytes(&bytes).unwrap();

        assert_eq!(decoded.request_id, 5678);
        assert!(decoded.reached);
        assert_eq!(decoded.signal_quality.len(), 2);
        assert_eq!(decoded.signal_quality[0].0, -80.0);
        assert_eq!(decoded.signal_quality[1].1, 12.0);
    }

    #[test]
    fn test_traceroute_start() {
        let mut tr = Traceroute::with_defaults(test_node_id(1));
        let request = tr.start_trace(test_node_id(10));

        assert_eq!(request.request_id, 1);
        assert_eq!(request.source.to_u32(), 1);
        assert_eq!(request.destination.to_u32(), 10);
        assert_eq!(tr.pending_count(), 1);
    }

    #[test]
    fn test_traceroute_intermediate_hop() {
        let tr = Traceroute::with_defaults(test_node_id(5));
        let request = RouteRequest::new(
            1,
            test_node_id(1),
            test_node_id(10),
            10,
        );

        match tr.handle_request(request, -80.0, 10.0) {
            RouteRequestResponse::Forward(req) => {
                assert_eq!(req.hop_count, 1);
                assert_eq!(req.route.len(), 2);
                assert_eq!(req.route[1].to_u32(), 5);
            }
            _ => panic!("Expected forward"),
        }
    }

    #[test]
    fn test_traceroute_destination() {
        let tr = Traceroute::with_defaults(test_node_id(10));
        let request = RouteRequest::new(
            1,
            test_node_id(1),
            test_node_id(10),
            10,
        );

        match tr.handle_request(request, -75.0, 15.0) {
            RouteRequestResponse::Reply(reply) => {
                assert!(reply.reached);
                assert_eq!(reply.signal_quality.len(), 1);
                assert_eq!(reply.signal_quality[0].0, -75.0);
            }
            _ => panic!("Expected reply"),
        }
    }

    #[test]
    fn test_traceroute_complete_flow() {
        // Node 1 initiates trace to Node 10
        let mut tr1 = Traceroute::with_defaults(test_node_id(1));
        let request = tr1.start_trace(test_node_id(10));

        // Node 5 is intermediate
        let tr5 = Traceroute::with_defaults(test_node_id(5));
        let response5 = tr5.handle_request(request, -80.0, 10.0);

        let request = match response5 {
            RouteRequestResponse::Forward(req) => req,
            _ => panic!("Expected forward"),
        };

        // Node 10 is destination
        let tr10 = Traceroute::with_defaults(test_node_id(10));
        let response10 = tr10.handle_request(request, -75.0, 12.0);

        let reply = match response10 {
            RouteRequestResponse::Reply(reply) => reply,
            _ => panic!("Expected reply"),
        };

        // Back at Node 1
        let result = tr1.handle_reply(reply).unwrap();

        assert!(result.reached);
        assert_eq!(result.hop_count(), 3); // 1 -> 5 -> 10
        assert_eq!(tr1.pending_count(), 0);
    }

    #[test]
    fn test_traceroute_result_format() {
        let mut result = TracerouteResult::new(1, test_node_id(1), test_node_id(10));
        result.hops.push(TracerouteHop {
            hop_number: 1,
            node_id: test_node_id(1),
            rtt: Some(Duration::from_millis(10)),
            rssi: Some(-80.0),
            snr: Some(10.0),
        });
        result.hops.push(TracerouteHop {
            hop_number: 2,
            node_id: test_node_id(5),
            rtt: Some(Duration::from_millis(25)),
            rssi: Some(-75.0),
            snr: Some(12.0),
        });
        result.reached = true;
        result.total_rtt = Some(Duration::from_millis(50));

        let output = result.format();
        assert!(output.contains("00000001"));
        assert!(output.contains("0000000a"));
        assert!(output.contains("reached"));
    }
}
