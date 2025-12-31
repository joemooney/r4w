//! Mesh Network Topology Visualization
//!
//! Interactive visualization of mesh network topology using r4w-core's
//! MeshSimulator for simulation and packet flow visualization.

use egui::{Color32, Pos2, Stroke, Ui, Vec2};
use r4w_core::mesh::{MeshSimulator, NodePosition, SimConfig};

/// State for the mesh network visualization
pub struct MeshNetworkView {
    /// Simulator instance
    simulator: MeshSimulator,
    /// Number of nodes
    node_count: usize,
    /// Simulation area dimensions
    area_width: f64,
    area_height: f64,
    /// Simulation step count
    step_count: u64,
    /// Auto-run simulation
    auto_run: bool,
    /// Steps per frame when auto-running
    steps_per_frame: u64,
    /// Message input
    message_input: String,
    /// Selected source node
    selected_source: usize,
    /// Show connection lines
    show_connections: bool,
    /// Transmission range for visualization
    tx_range: f64,
    /// Zoom level
    zoom: f32,
    /// Pan offset
    pan: Vec2,
}

impl Default for MeshNetworkView {
    fn default() -> Self {
        Self::new(8)
    }
}

impl MeshNetworkView {
    /// Create new mesh network view with specified number of nodes
    pub fn new(node_count: usize) -> Self {
        let area_width = 5000.0;
        let area_height = 5000.0;

        let config = SimConfig::default()
            .with_node_count(node_count)
            .with_area(area_width, area_height)
            .with_verbose(false);

        let simulator = MeshSimulator::new(config);

        Self {
            simulator,
            node_count,
            area_width,
            area_height,
            step_count: 0,
            auto_run: false,
            steps_per_frame: 1,
            message_input: "Hello Mesh!".to_string(),
            selected_source: 0,
            show_connections: true,
            tx_range: 2000.0,
            zoom: 1.0,
            pan: Vec2::ZERO,
        }
    }

    /// Recreate simulation with new parameters
    fn recreate_simulation(&mut self) {
        let config = SimConfig::default()
            .with_node_count(self.node_count)
            .with_area(self.area_width, self.area_height)
            .with_verbose(false);

        self.simulator = MeshSimulator::new(config);
        self.step_count = 0;
    }

    /// Render the mesh network view
    pub fn render(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.heading("Mesh Network Topology");
            ui.separator();
            ui.label(format!("Step: {}", self.step_count));
        });

        ui.separator();

        // Controls
        ui.horizontal(|ui| {
            // Node count
            ui.label("Nodes:");
            if ui.add(egui::DragValue::new(&mut self.node_count).range(2..=50)).changed() {
                self.recreate_simulation();
            }

            ui.separator();

            // Simulation controls
            if ui.button("Step").clicked() {
                self.simulator.step();
                self.step_count = self.simulator.step_count();
            }

            let run_text = if self.auto_run { "Stop" } else { "Run" };
            if ui.button(run_text).clicked() {
                self.auto_run = !self.auto_run;
            }

            if ui.button("Reset").clicked() {
                self.recreate_simulation();
            }

            ui.separator();

            // Speed control
            ui.label("Speed:");
            ui.add(egui::DragValue::new(&mut self.steps_per_frame).range(1..=100));
        });

        ui.separator();

        // Message sending controls
        ui.horizontal(|ui| {
            ui.label("Source:");
            egui::ComboBox::from_id_salt("source_node")
                .selected_text(format!("Node {}", self.selected_source))
                .show_ui(ui, |ui| {
                    for i in 0..self.node_count {
                        ui.selectable_value(&mut self.selected_source, i, format!("Node {}", i));
                    }
                });

            ui.label("Message:");
            ui.text_edit_singleline(&mut self.message_input);

            if ui.button("Send Broadcast").clicked() {
                self.simulator.send_message(self.selected_source, &self.message_input, None);
            }
        });

        ui.separator();

        // Visualization options
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.show_connections, "Show Connections");
            ui.label("TX Range:");
            ui.add(egui::DragValue::new(&mut self.tx_range).range(100.0..=5000.0).speed(50.0));
            ui.label("Zoom:");
            ui.add(egui::DragValue::new(&mut self.zoom).range(0.1..=5.0).speed(0.1));
        });

        ui.separator();

        // Auto-run simulation
        if self.auto_run {
            for _ in 0..self.steps_per_frame {
                self.simulator.step();
            }
            self.step_count = self.simulator.step_count();
            ui.ctx().request_repaint();
        }

        // Statistics
        let stats = self.simulator.stats();
        ui.horizontal(|ui| {
            ui.label(format!("TX: {}", stats.packets_transmitted));
            ui.label(format!("RX: {}", stats.packets_received));
            ui.label(format!("Collisions: {}", stats.collisions));
            if stats.packets_transmitted > 0 {
                let pdr = 100.0 * stats.packets_received as f64 / stats.packets_transmitted as f64;
                ui.label(format!("PDR: {:.1}%", pdr));
            }
        });

        ui.separator();

        // Network topology visualization
        let available_size = ui.available_size();
        let (response, painter) = ui.allocate_painter(available_size, egui::Sense::drag());

        // Handle pan with drag
        if response.dragged() {
            self.pan += response.drag_delta();
        }

        // Handle zoom with scroll
        let scroll_delta = ui.input(|i| i.raw_scroll_delta);
        if scroll_delta.y != 0.0 {
            self.zoom = (self.zoom + scroll_delta.y * 0.001).clamp(0.1, 5.0);
        }

        let rect = response.rect;
        let center = rect.center();

        // Background
        painter.rect_filled(rect, 0.0, Color32::from_gray(30));

        // Scale factor to fit simulation area in view
        let scale_x = (rect.width() * 0.9) / self.area_width as f32 * self.zoom;
        let scale_y = (rect.height() * 0.9) / self.area_height as f32 * self.zoom;
        let scale = scale_x.min(scale_y);

        // Helper to convert simulation coords to screen coords
        let to_screen = |pos: &NodePosition| -> Pos2 {
            let x = (pos.x as f32 - (self.area_width as f32 / 2.0)) * scale;
            let y = (pos.y as f32 - (self.area_height as f32 / 2.0)) * scale;
            Pos2::new(center.x + x + self.pan.x, center.y + y + self.pan.y)
        };

        // Draw connections (if enabled)
        if self.show_connections {
            for i in 0..self.node_count {
                if let Some(pos_i) = self.simulator.node_position(i) {
                    for j in (i + 1)..self.node_count {
                        if let Some(pos_j) = self.simulator.node_position(j) {
                            let dist = ((pos_i.x - pos_j.x).powi(2) + (pos_i.y - pos_j.y).powi(2)).sqrt();
                            if dist < self.tx_range {
                                // Color based on distance (green = close, red = far)
                                let ratio = (dist / self.tx_range) as f32;
                                let r = (ratio * 200.0) as u8;
                                let g = ((1.0 - ratio) * 200.0) as u8;
                                let color = Color32::from_rgb(r, g, 50);
                                painter.line_segment(
                                    [to_screen(&pos_i), to_screen(&pos_j)],
                                    Stroke::new(1.0, color.gamma_multiply(0.5)),
                                );
                            }
                        }
                    }
                }
            }
        }

        // Draw nodes
        let node_radius = 12.0 * self.zoom;
        for i in 0..self.node_count {
            if let (Some(pos), Some(node_id)) = (self.simulator.node_position(i), self.simulator.node_id(i)) {
                let screen_pos = to_screen(&pos);

                // Node color (selected = green, others = blue)
                let color = if i == self.selected_source {
                    Color32::from_rgb(50, 200, 50)
                } else {
                    Color32::from_rgb(50, 100, 200)
                };

                // Draw node circle
                painter.circle_filled(screen_pos, node_radius, color);
                painter.circle_stroke(screen_pos, node_radius, Stroke::new(2.0, Color32::WHITE));

                // Draw node label
                let label = format!("{}", i);
                painter.text(
                    screen_pos,
                    egui::Align2::CENTER_CENTER,
                    label,
                    egui::FontId::proportional(10.0 * self.zoom),
                    Color32::WHITE,
                );

                // Draw node ID below
                let id_text = format!("{:04x}", node_id.to_u32() & 0xFFFF);
                painter.text(
                    Pos2::new(screen_pos.x, screen_pos.y + node_radius + 8.0),
                    egui::Align2::CENTER_TOP,
                    id_text,
                    egui::FontId::proportional(8.0 * self.zoom),
                    Color32::GRAY,
                );
            }
        }

        // Instructions
        ui.vertical(|ui| {
            ui.label("Drag to pan, scroll to zoom");
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mesh_network_view_creation() {
        let view = MeshNetworkView::new(5);
        assert_eq!(view.node_count, 5);
        assert_eq!(view.step_count, 0);
    }

    #[test]
    fn test_recreate_simulation() {
        let mut view = MeshNetworkView::new(5);
        view.node_count = 10;
        view.recreate_simulation();
        // Verify it doesn't panic
        assert_eq!(view.step_count, 0);
    }
}
