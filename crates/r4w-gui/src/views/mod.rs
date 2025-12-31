//! View modules for the LoRa Explorer application

mod adsb;
mod ale;
mod chirp;
pub mod code_explorer;
mod constellation;
mod demod;
mod fhss;
mod generic_demod;
mod generic_mod;
mod generic_pipeline;
mod mesh_network;
mod modulation;
mod overview;
mod performance;
mod pipeline;
mod spectrum;
mod stanag;
mod remote_lab;
mod streaming;
mod udp_benchmark;
mod waveform;
mod waveform_comparison;
mod waveform_wizard;

pub use adsb::AdsbView;
pub use ale::AleView;
pub use chirp::ChirpView;
pub use code_explorer::CodeExplorerView;
pub use constellation::ConstellationView;
pub use demod::DemodView;
pub use fhss::FhssView;
pub use generic_demod::GenericDemodulationView;
pub use generic_mod::GenericModulationView;
pub use generic_pipeline::GenericPipelineView;
pub use mesh_network::MeshNetworkView;
pub use modulation::ModulationView;
pub use overview::OverviewView;
pub use performance::PerformanceView;
pub use pipeline::PipelineView;
pub use remote_lab::RemoteLabView;
pub use spectrum::SpectrumView;
pub use stanag::Stanag4285View;
pub use streaming::StreamingView;
pub use udp_benchmark::UdpBenchmarkView;
pub use waveform::{WaveformView, WaveformParams};
pub use waveform_comparison::WaveformComparisonView;
pub use waveform_wizard::WaveformWizardView;

use egui::Ui;
use r4w_core::params::LoRaParams;
use r4w_core::types::IQSample;

/// Common trait for views
pub trait View {
    fn render(&mut self, ui: &mut Ui, params: &LoRaParams, samples: &Option<Vec<IQSample>>);
}
