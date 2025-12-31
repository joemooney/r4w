//! Main application state and UI coordination

use eframe::egui;
use r4w_core::{
    demodulation::Demodulator,
    modulation::Modulator,
    params::LoRaParams,
    types::IQSample,
    waveform::WaveformFactory,
};
use r4w_sim::{Channel, ChannelConfig, ChannelModel};

use crate::platform::{platform, PlatformServices};
use crate::streaming::{PlaybackState, StreamConfig, StreamManager};
use crate::views::{
    AdsbView, AleView, ChirpView, CodeExplorerView, ConstellationView, DemodView,
    FhssView, GenericDemodulationView, GenericModulationView, GenericPipelineView,
    MeshNetworkView, ModulationView, OverviewView, PerformanceView, PipelineView, RemoteLabView,
    SpectrumView, Stanag4285View, StreamingView, UdpBenchmarkView, WaveformComparisonView,
    WaveformView, WaveformParams, WaveformWizardView,
};

/// Waveform group for visual organization in dropdown
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaveformGroup {
    Simple,
    Pulse,
    Digital,
    HighOrder,
    Analog,
    MultiCarrier,
    SpreadSpectrum,
    IoTRadar,
    HfMilitary,
}

impl WaveformGroup {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Simple => "Simple",
            Self::Pulse => "Pulse",
            Self::Digital => "Digital",
            Self::HighOrder => "High-Order",
            Self::Analog => "Analog",
            Self::MultiCarrier => "Multi-Carrier",
            Self::SpreadSpectrum => "Spread Spectrum",
            Self::IoTRadar => "IoT & Radar",
            Self::HfMilitary => "HF/Military",
        }
    }

    /// Get all waveforms in this group
    pub fn waveforms(&self) -> &[&str] {
        match self {
            Self::Simple => &["CW"],
            Self::Pulse => &["OOK", "PPM", "ADS-B"],
            Self::Digital => &["BFSK", "4-FSK", "BPSK", "QPSK", "8-PSK"],
            Self::HighOrder => &["16-QAM", "64-QAM", "256-QAM"],
            Self::Analog => &["AM", "FM"],
            Self::MultiCarrier => &["OFDM"],
            Self::SpreadSpectrum => &["DSSS", "DSSS-QPSK", "FHSS", "LoRa"],
            Self::IoTRadar => &["Zigbee", "UWB", "FMCW"],
            Self::HfMilitary => &["STANAG-4285", "ALE", "SINCGARS", "HAVEQUICK", "Link-16", "MIL-STD-188-110", "P25"],
        }
    }

    /// Get all groups in display order
    pub fn all() -> &'static [WaveformGroup] {
        &[
            WaveformGroup::Simple,
            WaveformGroup::Pulse,
            WaveformGroup::Digital,
            WaveformGroup::HighOrder,
            WaveformGroup::Analog,
            WaveformGroup::MultiCarrier,
            WaveformGroup::SpreadSpectrum,
            WaveformGroup::IoTRadar,
            WaveformGroup::HfMilitary,
        ]
    }

    /// Get all waveforms in order (flattened from all groups)
    pub fn all_waveforms() -> Vec<&'static str> {
        Self::all()
            .iter()
            .flat_map(|g| g.waveforms().iter().copied())
            .collect()
    }

    /// Get the group for a given waveform name
    pub fn for_waveform(name: &str) -> Option<WaveformGroup> {
        for group in Self::all() {
            if group.waveforms().contains(&name) {
                return Some(*group);
            }
        }
        None
    }
}

/// Check if a waveform is LoRa (for view filtering)
fn is_lora_waveform(name: &str) -> bool {
    name == "LoRa"
}

/// Currently active view
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveView {
    Overview,
    Waveforms,
    WaveformWizard,
    FhssLab,
    Stanag4285Lab,
    AleLab,
    MeshNetwork,
    Streaming,
    UdpBenchmark,
    RemoteLab,
    CodeExplorer,
    AdsbDecoder,
    Chirp,
    Modulation,
    Demodulation,
    Pipeline,
    Spectrum,
    Constellation,
    Performance,
    WaveformComparison,
}

impl ActiveView {
    pub fn name(&self) -> &str {
        match self {
            Self::Overview => "Overview",
            Self::Waveforms => "Waveform Lab",
            Self::WaveformWizard => "Waveform Wizard",
            Self::FhssLab => "FHSS Lab",
            Self::Stanag4285Lab => "STANAG 4285",
            Self::AleLab => "ALE",
            Self::MeshNetwork => "Mesh Network",
            Self::Streaming => "Streaming",
            Self::UdpBenchmark => "UDP Benchmark",
            Self::RemoteLab => "Remote Lab",
            Self::CodeExplorer => "Code Explorer",
            Self::AdsbDecoder => "ADS-B Decoder",
            Self::Chirp => "Chirp Signals",
            Self::Modulation => "Modulation",
            Self::Demodulation => "Demodulation",
            Self::Pipeline => "Full Pipeline",
            Self::Spectrum => "Spectrum Analyzer",
            Self::Constellation => "Constellation",
            Self::Performance => "Performance",
            Self::WaveformComparison => "Waveform Comparison",
        }
    }

    pub fn description(&self) -> &str {
        match self {
            Self::Overview => "Introduction to SDR and waveform concepts",
            Self::Waveforms => "Explore different modulation schemes",
            Self::WaveformWizard => "Create custom waveform specifications with guided wizard",
            Self::FhssLab => "Frequency hopping spread spectrum with anti-jam demo",
            Self::Stanag4285Lab => "NATO HF data modem (75-3600 bps PSK)",
            Self::AleLab => "Automatic Link Establishment (8-FSK, Golay FEC)",
            Self::MeshNetwork => "Interactive mesh network topology and packet flow simulation",
            Self::Streaming => "Real-time signal visualization with oscilloscope and waterfall",
            Self::UdpBenchmark => "Benchmark waveform processing with UDP input",
            Self::RemoteLab => "Control remote Raspberry Pi agents for distributed TX/RX testing",
            Self::CodeExplorer => "View implementation with syntax highlighting",
            Self::AdsbDecoder => "Decode and analyze ADS-B Mode S messages",
            Self::Chirp => "Explore chirp signal generation and properties",
            Self::Modulation => "Understand the modulation process",
            Self::Demodulation => "See how symbols are extracted from signals",
            Self::Pipeline => "Complete TX/RX pipeline visualization",
            Self::Spectrum => "Frequency domain analysis",
            Self::Constellation => "I/Q signal visualization",
            Self::Performance => "Compare sequential vs parallel performance",
            Self::WaveformComparison => "Benchmark comparison of BPSK, QPSK, and LoRa waveforms",
        }
    }

    /// Check if this view is available for a given waveform
    pub fn is_for_waveform(&self, waveform: &str) -> bool {
        match self {
            Self::Overview => true,
            Self::Waveforms => true,
            Self::WaveformWizard => true, // Wizard always available
            Self::FhssLab => true, // FHSS Lab always available (has own FHSS instance)
            Self::Stanag4285Lab => true, // STANAG 4285 Lab always available
            Self::AleLab => true, // ALE Lab always available
            Self::MeshNetwork => true, // Mesh network always available
            Self::Streaming => true,
            Self::UdpBenchmark => true,
            Self::RemoteLab => true,
            Self::CodeExplorer => true,
            Self::Spectrum => true,
            Self::Constellation => true,
            Self::Performance => true, // Performance view available for all waveforms
            Self::WaveformComparison => true, // Comparison view available for all waveforms
            // ADS-B specific view
            Self::AdsbDecoder => waveform == "ADS-B",
            // Chirp is LoRa-specific (chirp signal visualization)
            Self::Chirp => is_lora_waveform(waveform),
            // Modulation/Demodulation/Pipeline available for all waveforms
            Self::Modulation | Self::Demodulation | Self::Pipeline => true,
        }
    }
}

/// Main application state
pub struct WaveformExplorer {
    /// Selected waveform name (from grouped dropdown)
    selected_waveform: String,

    /// Currently active view
    active_view: ActiveView,

    /// LoRa parameters (for LoRa mode)
    params: LoRaParams,

    /// Spreading factor selection
    sf_value: u8,

    /// Bandwidth selection (kHz)
    bw_khz: u32,

    /// Coding rate
    cr_value: u8,

    /// Channel SNR
    snr_db: f32,

    /// Channel model
    channel_model: ChannelModel,

    /// Enable CFO simulation
    cfo_enabled: bool,
    cfo_hz: f32,

    /// Test payload
    payload: String,

    /// Generated samples (for visualization)
    generated_samples: Option<Vec<IQSample>>,

    /// Current modulator (LoRa mode)
    modulator: Option<Modulator>,

    /// Current demodulator (LoRa mode)
    demodulator: Option<Demodulator>,

    /// Channel simulator
    channel: Channel,

    /// Views
    overview_view: OverviewView,
    waveform_view: WaveformView,
    waveform_wizard_view: WaveformWizardView,
    fhss_view: FhssView,
    stanag_view: Stanag4285View,
    ale_view: AleView,
    mesh_network_view: MeshNetworkView,
    streaming_view: StreamingView,
    udp_benchmark_view: UdpBenchmarkView,
    remote_lab_view: RemoteLabView,
    code_explorer_view: CodeExplorerView,
    adsb_view: AdsbView,
    chirp_view: ChirpView,
    modulation_view: ModulationView,
    demodulation_view: DemodView,
    pipeline_view: PipelineView,
    spectrum_view: SpectrumView,
    constellation_view: ConstellationView,
    performance_view: PerformanceView,
    waveform_comparison_view: WaveformComparisonView,

    /// Generic views (for non-LoRa waveforms)
    generic_mod_view: GenericModulationView,
    generic_demod_view: GenericDemodulationView,
    generic_pipeline_view: GenericPipelineView,

    /// Streaming manager for real-time playback
    stream_manager: StreamManager,

    /// Auto-update on parameter change
    auto_update: bool,

    // ============== Waveform Parameters (General Mode) ==============

    /// Common: Sample rate (Hz)
    wf_sample_rate: f64,
    /// Common: Signal amplitude (0.0 - 1.0)
    wf_amplitude: f32,

    /// CW: Tone frequency (Hz)
    wf_cw_frequency: f64,
    /// CW: Duration (ms)
    wf_cw_duration_ms: f64,

    /// Digital waveforms: Symbol rate (symbols/sec)
    wf_symbol_rate: f64,
    /// Digital waveforms: Carrier frequency (Hz)
    wf_carrier_freq: f64,
    /// FSK/FM: Frequency deviation (Hz)
    wf_fsk_deviation: f64,

    /// AM: Modulation index (0.0 to 1.0+)
    wf_am_mod_index: f64,
    /// AM: Suppress carrier (DSB-SC mode)
    wf_am_suppress_carrier: bool,

    /// PPM: Use ADS-B variant
    #[allow(dead_code)]
    wf_ppm_adsb_mode: bool,

    /// OFDM: FFT size
    wf_ofdm_fft_size: usize,
    /// OFDM: Number of data subcarriers
    wf_ofdm_data_subcarriers: usize,
    /// OFDM: Cyclic prefix ratio (0.0 to 0.5)
    wf_ofdm_cp_ratio: f64,
    /// OFDM: Subcarrier modulation (0=BPSK, 1=QPSK, 2=16-QAM, 3=64-QAM)
    wf_ofdm_subcarrier_mod: usize,

    /// DSSS: PN sequence degree (5-10, determines chips per symbol: 31-1023)
    wf_dsss_pn_degree: u8,
    /// DSSS: Modulation type (0=BPSK, 1=QPSK)
    wf_dsss_modulation: usize,
    /// DSSS: Samples per chip (oversampling factor)
    wf_dsss_samples_per_chip: usize,

    /// FHSS: Number of frequency channels
    wf_fhss_num_channels: usize,
    /// FHSS: Channel spacing in Hz
    wf_fhss_channel_spacing: f64,
    /// FHSS: Hop rate (hops per second)
    wf_fhss_hop_rate: f64,
    /// FHSS: Symbols per hop
    wf_fhss_symbols_per_hop: usize,
    /// FHSS: Modulation type (0=BFSK, 1=BPSK, 2=QPSK)
    wf_fhss_modulation: usize,
    /// FHSS: Hop pattern (0=PseudoRandom, 1=Sequential)
    wf_fhss_pattern: usize,

    /// Zigbee: Samples per chip (oversampling)
    wf_zigbee_samples_per_chip: usize,
    /// Zigbee: Enable half-sine pulse shaping
    wf_zigbee_half_sine: bool,

    /// UWB: Pulse shape (0=GaussianMonocycle, 1=GaussianDoublet, 2=RaisedCosine, 3=Rectangular)
    wf_uwb_pulse_shape: usize,
    /// UWB: Modulation (0=OOK, 1=BPSK, 2=PPM)
    wf_uwb_modulation: usize,
    /// UWB: Pulse duration in nanoseconds
    wf_uwb_pulse_duration_ns: f64,
    /// UWB: Pulse interval in nanoseconds
    wf_uwb_pulse_interval_ns: f64,
    /// UWB: Pulses per bit (integration gain)
    wf_uwb_pulses_per_bit: usize,

    /// FMCW: Chirp bandwidth in MHz
    wf_fmcw_bandwidth_mhz: f64,
    /// FMCW: Chirp duration in microseconds
    wf_fmcw_chirp_duration_us: f64,
    /// FMCW: Number of chirps
    wf_fmcw_num_chirps: usize,
    /// FMCW: Chirp direction (0=Up, 1=Down, 2=Triangle, 3=Sawtooth)
    wf_fmcw_chirp_direction: usize,

    /// General mode: SNR for channel simulation
    wf_snr_db: f32,
    /// General mode: Channel model
    wf_channel_model: ChannelModel,

    /// General mode: Test bit pattern
    wf_test_bits: String,

    /// General mode: Generated samples for info display
    // REMOVED: wf_generated_samples - now using unified generated_samples for all waveforms
    /// General mode: Demodulated bits (for BER calculation)
    wf_demod_bits: Option<Vec<u8>>,
    /// General mode: BER result
    wf_ber: Option<f64>,
}

impl WaveformExplorer {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let params = LoRaParams::builder()
            .spreading_factor(7)
            .bandwidth(125_000)
            .coding_rate(1)
            .oversample(4)
            .build();

        let channel_config = ChannelConfig {
            snr_db: 20.0,
            sample_rate: params.sample_rate,
            ..Default::default()
        };

        Self {
            selected_waveform: "BPSK".to_string(),
            active_view: ActiveView::Overview,
            params: params.clone(),
            sf_value: 7,
            bw_khz: 125,
            cr_value: 1,
            snr_db: 20.0,
            channel_model: ChannelModel::Awgn,
            cfo_enabled: false,
            cfo_hz: 0.0,
            payload: "Hello!".to_string(),
            generated_samples: None,
            modulator: Some(Modulator::new(params.clone())),
            demodulator: Some(Demodulator::new(params)),
            channel: Channel::new(channel_config),
            overview_view: OverviewView::new(),
            waveform_view: WaveformView::new(),
            waveform_wizard_view: WaveformWizardView::new(),
            fhss_view: FhssView::new(),
            stanag_view: Stanag4285View::new(),
            ale_view: AleView::new(),
            mesh_network_view: MeshNetworkView::default(),
            performance_view: PerformanceView::new(),
            streaming_view: StreamingView::new(),
            udp_benchmark_view: UdpBenchmarkView::new(),
            remote_lab_view: RemoteLabView::new(),
            code_explorer_view: CodeExplorerView::new(),
            adsb_view: AdsbView::new(),
            chirp_view: ChirpView::new(),
            modulation_view: ModulationView::new(),
            demodulation_view: DemodView::new(),
            pipeline_view: PipelineView::new(),
            spectrum_view: SpectrumView::new(),
            constellation_view: ConstellationView::new(),
            waveform_comparison_view: WaveformComparisonView::new(),
            generic_mod_view: GenericModulationView::new(),
            generic_demod_view: GenericDemodulationView::new(),
            generic_pipeline_view: GenericPipelineView::new(),
            stream_manager: StreamManager::new(StreamConfig::default()),
            auto_update: true,

            // Waveform parameters (General mode)
            wf_sample_rate: 48000.0,
            wf_amplitude: 1.0,
            wf_cw_frequency: 1000.0,
            wf_cw_duration_ms: 100.0,
            wf_symbol_rate: 1000.0,
            wf_carrier_freq: 10000.0,
            wf_fsk_deviation: 500.0,
            wf_am_mod_index: 0.8,
            wf_am_suppress_carrier: false,
            wf_ppm_adsb_mode: false,
            wf_ofdm_fft_size: 64,
            wf_ofdm_data_subcarriers: 48,
            wf_ofdm_cp_ratio: 0.25,
            wf_ofdm_subcarrier_mod: 1, // QPSK
            wf_dsss_pn_degree: 7,      // 127 chips = 21 dB processing gain
            wf_dsss_modulation: 0,     // BPSK
            wf_dsss_samples_per_chip: 4,
            wf_fhss_num_channels: 50,
            wf_fhss_channel_spacing: 25000.0, // 25 kHz
            wf_fhss_hop_rate: 100.0,          // 100 hops/sec
            wf_fhss_symbols_per_hop: 10,
            wf_fhss_modulation: 0,     // BFSK
            wf_fhss_pattern: 0,        // PseudoRandom
            wf_zigbee_samples_per_chip: 4,
            wf_zigbee_half_sine: true,
            wf_uwb_pulse_shape: 0,     // GaussianMonocycle
            wf_uwb_modulation: 1,      // BPSK
            wf_uwb_pulse_duration_ns: 2.0,
            wf_uwb_pulse_interval_ns: 100.0,
            wf_uwb_pulses_per_bit: 1,
            wf_fmcw_bandwidth_mhz: 150.0,
            wf_fmcw_chirp_duration_us: 40.0,
            wf_fmcw_num_chirps: 4,
            wf_fmcw_chirp_direction: 3, // Sawtooth
            wf_snr_db: 20.0,
            wf_channel_model: ChannelModel::Awgn,
            wf_test_bits: "10110010".to_string(),
            // wf_generated_samples removed - using unified generated_samples
            wf_demod_bits: None,
            wf_ber: None,
        }
    }

    /// Update LoRa parameters from UI values
    fn update_params(&mut self) {
        self.params = LoRaParams::builder()
            .spreading_factor(self.sf_value)
            .bandwidth(self.bw_khz * 1000)
            .coding_rate(self.cr_value)
            .oversample(4)
            .build();

        self.modulator = Some(Modulator::new(self.params.clone()));
        self.demodulator = Some(Demodulator::new(self.params.clone()));

        // Update channel
        let mut ch_config = self.channel.config().clone();
        ch_config.snr_db = self.snr_db as f64;
        ch_config.sample_rate = self.params.sample_rate;
        ch_config.model = self.channel_model;
        if self.cfo_enabled {
            ch_config.cfo_hz = self.cfo_hz as f64;
        } else {
            ch_config.cfo_hz = 0.0;
        }
        self.channel.set_config(ch_config);

        if self.auto_update {
            self.generate_signal();
        }
    }

    /// Generate signal with current settings
    fn generate_signal(&mut self) {
        if let Some(ref mut modulator) = self.modulator {
            modulator.enable_stage_recording();
            let samples = modulator.modulate(self.payload.as_bytes());
            self.generated_samples = Some(samples);
        }
    }

    /// Get test data as bytes from the test bits string
    fn get_test_data(&self) -> Vec<u8> {
        // Convert binary string to bytes
        let bits: Vec<u8> = self.wf_test_bits.chars()
            .filter_map(|c| match c {
                '0' => Some(0u8),
                '1' => Some(1u8),
                _ => None,
            })
            .collect();

        // Pack bits into bytes
        bits.chunks(8)
            .map(|chunk| {
                chunk.iter()
                    .enumerate()
                    .fold(0u8, |acc, (i, &bit)| acc | (bit << (7 - i)))
            })
            .collect()
    }

    /// Navigate to the next waveform in the list
    fn next_waveform(&mut self) {
        let all_waveforms = WaveformGroup::all_waveforms();
        if let Some(idx) = all_waveforms.iter().position(|w| *w == self.selected_waveform) {
            let next_idx = (idx + 1) % all_waveforms.len();
            self.set_waveform(all_waveforms[next_idx]);
        }
    }

    /// Navigate to the previous waveform in the list
    fn prev_waveform(&mut self) {
        let all_waveforms = WaveformGroup::all_waveforms();
        if let Some(idx) = all_waveforms.iter().position(|w| *w == self.selected_waveform) {
            let prev_idx = if idx == 0 { all_waveforms.len() - 1 } else { idx - 1 };
            self.set_waveform(all_waveforms[prev_idx]);
        }
    }

    /// Set the current waveform and update related state
    fn set_waveform(&mut self, waveform: &str) {
        if self.selected_waveform != waveform {
            self.selected_waveform = waveform.to_string();

            // Clear samples when switching waveforms
            self.generated_samples = None;

            // Sync streaming view generator with sidebar selection
            self.streaming_view.sync_with_sidebar(waveform);

            // Check view compatibility
            if !self.active_view.is_for_waveform(waveform) {
                self.active_view = ActiveView::Overview;
            }
        }
    }

    /// Get the path to the tutorial HTML file
    fn get_tutorial_path() -> Option<std::path::PathBuf> {
        // Try relative path from executable
        if let Ok(exe_path) = std::env::current_exe() {
            if let Some(exe_dir) = exe_path.parent() {
                // Check various relative locations
                let paths = [
                    exe_dir.join("../../tutorial/index.html"),
                    exe_dir.join("../../../tutorial/index.html"),
                    exe_dir.join("tutorial/index.html"),
                ];
                for path in paths {
                    if path.exists() {
                        return Some(path);
                    }
                }
            }
        }

        // Try from current working directory
        let cwd_path = std::path::PathBuf::from("docs/tutorial/index.html");
        if cwd_path.exists() {
            return Some(cwd_path);
        }

        None
    }

    /// Open the tutorial in the default browser
    fn open_tutorial() {
        #[cfg(not(target_arch = "wasm32"))]
        {
            if let Some(path) = Self::get_tutorial_path() {
                if let Ok(abs_path) = path.canonicalize() {
                    let url = format!("file://{}", abs_path.display());
                    platform().open_url(&url);
                }
            } else {
                // Fallback: try to open from project root
                let fallback = std::path::PathBuf::from("tutorial/index.html");
                if let Ok(abs_path) = fallback.canonicalize() {
                    let url = format!("file://{}", abs_path.display());
                    platform().open_url(&url);
                }
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            // On web, open the GitHub-hosted tutorial in a new tab
            // The tutorial HTML is too large to embed, so we link to the repo
            platform().open_url("https://github.com/joemooney/ai-sdr-lora/blob/master/tutorial/index.html");
        }
    }

    /// Render the side panel with navigation and parameters
    fn render_side_panel(&mut self, ctx: &egui::Context) {
        // Navigation state - must be outside the closure for borrow checker
        let mut nav_prev = false;
        let mut nav_next = false;
        let mut waveform_changed: Option<String> = None;

        egui::SidePanel::left("nav_panel")
            .default_width(280.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.heading("Waveform Explorer");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Tutorial").clicked() {
                            Self::open_tutorial();
                        }
                    });
                });
                ui.separator();

                // Waveform Selection (single grouped dropdown with navigation)
                ui.heading("Waveform");
                ui.add_space(4.0);

                // Get group name for current waveform
                let current_group = match WaveformGroup::for_waveform(&self.selected_waveform) {
                    Some(g) => g.name(),
                    None => "Unknown",
                };
                let display_text = format!("{} ({})", self.selected_waveform, current_group);

                // Navigation row with prev/next buttons and dropdown
                ui.horizontal(|ui| {
                    // Previous button
                    if ui.button("◀").on_hover_text("Previous waveform (Ctrl+Left)").clicked() {
                        nav_prev = true;
                    }

                    // Waveform dropdown
                    egui::ComboBox::from_id_salt("waveform_select")
                        .selected_text(&display_text)
                        .width(150.0)
                        .show_ui(ui, |ui| {
                            for group in WaveformGroup::all() {
                                // Group header (non-selectable)
                                ui.label(egui::RichText::new(group.name()).strong().color(egui::Color32::LIGHT_GRAY));
                                ui.add_space(2.0);

                                // Waveforms in this group
                                for wf_name in group.waveforms() {
                                    ui.horizontal(|ui| {
                                        ui.add_space(12.0); // Indent
                                        if ui.selectable_value(
                                            &mut self.selected_waveform,
                                            wf_name.to_string(),
                                            *wf_name
                                        ).clicked() {
                                            waveform_changed = Some(wf_name.to_string());
                                        }
                                    });
                                }
                                ui.add_space(4.0);
                            }
                        });

                    // Next button
                    if ui.button("▶").on_hover_text("Next waveform (Ctrl+Right)").clicked() {
                        nav_next = true;
                    }
                });

                ui.add_space(12.0);
                ui.separator();

                // Navigation (filtered by waveform type)
                ui.heading("Navigation");
                ui.spacing_mut().item_spacing.y = 4.0;

                let all_views = [
                    ActiveView::Overview,
                    ActiveView::Waveforms,
                    ActiveView::WaveformWizard,
                    ActiveView::Streaming,
                    ActiveView::UdpBenchmark,
                    ActiveView::RemoteLab,
                    ActiveView::WaveformComparison,
                    ActiveView::CodeExplorer,
                    ActiveView::Performance,
                    ActiveView::AdsbDecoder,
                    ActiveView::Chirp,
                    ActiveView::Modulation,
                    ActiveView::Demodulation,
                    ActiveView::Pipeline,
                    ActiveView::Spectrum,
                    ActiveView::Constellation,
                ];

                for view in all_views {
                    if view.is_for_waveform(&self.selected_waveform) {
                        let selected = self.active_view == view;
                        if ui.selectable_label(selected, view.name()).clicked() {
                            self.active_view = view;
                        }
                    }
                }

                ui.add_space(20.0);
                ui.separator();

                // Show waveform-specific parameters
                if is_lora_waveform(&self.selected_waveform) {
                    self.render_lora_params(ui);
                } else {
                    self.render_general_params(ui);
                }
                }); // end ScrollArea
            });

        // Handle keyboard shortcuts for waveform navigation
        ctx.input(|i| {
            if i.modifiers.ctrl || i.modifiers.command {
                if i.key_pressed(egui::Key::ArrowLeft) {
                    nav_prev = true;
                }
                if i.key_pressed(egui::Key::ArrowRight) {
                    nav_next = true;
                }
            }
        });

        // Handle navigation after the panel (outside closure)
        if nav_prev {
            self.prev_waveform();
        } else if nav_next {
            self.next_waveform();
        }
        if let Some(wf) = waveform_changed {
            self.set_waveform(&wf);
        }
    }

    /// Render parameters for general waveforms
    fn render_general_params(&mut self, ui: &mut egui::Ui) {
        // Dispatch to waveform-specific parameter UI
        match self.selected_waveform.as_str() {
            "CW" => self.render_cw_params(ui),
            "OOK" => self.render_ook_params(ui),
            "AM" | "4-AM" => self.render_am_params(ui),
            "FM" | "4-FM" => self.render_fm_params(ui),
            "PPM" | "ADS-B" => self.render_ppm_params(ui),
            "BFSK" | "4-FSK" => self.render_fsk_params(ui),
            "BPSK" | "QPSK" | "8-PSK" => self.render_psk_params(ui),
            "16-QAM" | "64-QAM" | "256-QAM" => self.render_qam_params(ui),
            "OFDM" => self.render_ofdm_params(ui),
            "DSSS" | "DSSS-QPSK" => self.render_dsss_params(ui),
            "FHSS" => self.render_fhss_params(ui),
            "Zigbee" | "802.15.4" => self.render_zigbee_params(ui),
            "UWB" => self.render_uwb_params(ui),
            "FMCW" => self.render_fmcw_params(ui),
            _ => self.render_default_waveform_params(ui),
        }
    }

    /// Render CW (Continuous Wave) parameters - simplest waveform
    fn render_cw_params(&mut self, ui: &mut egui::Ui) {
        ui.heading("CW Parameters");
        ui.add_space(8.0);

        let mut params_changed = false;

        // Frequency
        ui.horizontal(|ui| {
            ui.label("Frequency (Hz):");
        });
        let freq_slider = egui::Slider::new(&mut self.wf_cw_frequency, 100.0..=10000.0)
            .logarithmic(true)
            .suffix(" Hz");
        if ui.add(freq_slider).changed() {
            params_changed = true;
        }

        // Duration
        ui.horizontal(|ui| {
            ui.label("Duration (ms):");
        });
        let dur_slider = egui::Slider::new(&mut self.wf_cw_duration_ms, 10.0..=1000.0)
            .suffix(" ms");
        if ui.add(dur_slider).changed() {
            params_changed = true;
        }

        // Sample rate
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label("Sample Rate:");
        });
        egui::ComboBox::from_id_salt("cw_sample_rate")
            .selected_text(format!("{} Hz", self.wf_sample_rate as u32))
            .show_ui(ui, |ui| {
                for rate in [8000.0, 16000.0, 44100.0, 48000.0, 96000.0] {
                    if ui.selectable_value(&mut self.wf_sample_rate, rate, format!("{} Hz", rate as u32)).changed() {
                        params_changed = true;
                    }
                }
            });

        ui.add_space(12.0);
        ui.separator();

        // Generate button
        ui.heading("Generate");
        ui.add_space(8.0);

        ui.horizontal(|ui| {
            let needs_initial = self.auto_update && self.generated_samples.is_none();
            if ui.button("Generate Signal").clicked() || (self.auto_update && params_changed) || needs_initial {
                self.generate_cw_signal();
            }
            ui.checkbox(&mut self.auto_update, "Auto-update");
        });

        ui.add_space(12.0);
        ui.separator();

        // Signal Info
        ui.heading("Signal Info");
        ui.add_space(8.0);

        if let Some(ref samples) = self.generated_samples {
            let duration_ms = samples.len() as f64 / self.wf_sample_rate * 1000.0;
            ui.label(format!("Samples: {}", samples.len()));
            ui.label(format!("Duration: {:.2} ms", duration_ms));
            ui.label(format!("Frequency: {:.0} Hz", self.wf_cw_frequency));

            // Show cycles
            let cycles = self.wf_cw_frequency * duration_ms / 1000.0;
            ui.label(format!("Cycles: {:.1}", cycles));
        } else {
            ui.label("No signal generated");
            ui.label("Click 'Generate Signal' to create");
        }
    }

    /// Generate CW signal
    fn generate_cw_signal(&mut self) {
        use std::f64::consts::PI;

        let num_samples = (self.wf_sample_rate * self.wf_cw_duration_ms / 1000.0) as usize;
        let mut samples = Vec::with_capacity(num_samples);

        for i in 0..num_samples {
            let t = i as f64 / self.wf_sample_rate;
            let phase = 2.0 * PI * self.wf_cw_frequency * t;
            let sample = IQSample::new(
                phase.cos() * self.wf_amplitude as f64,
                phase.sin() * self.wf_amplitude as f64,
            );
            samples.push(sample);
        }

        self.generated_samples = Some(samples);
        self.wf_demod_bits = None;
        self.wf_ber = None;
    }

    /// Render OOK (On-Off Keying) parameters
    fn render_ook_params(&mut self, ui: &mut egui::Ui) {
        ui.heading("OOK Parameters");
        ui.add_space(8.0);

        let mut params_changed = false;

        // Symbol rate
        ui.horizontal(|ui| {
            ui.label("Symbol Rate:");
        });
        let sym_slider = egui::Slider::new(&mut self.wf_symbol_rate, 100.0..=10000.0)
            .logarithmic(true)
            .suffix(" sym/s");
        if ui.add(sym_slider).changed() {
            params_changed = true;
        }

        // Carrier frequency
        ui.horizontal(|ui| {
            ui.label("Carrier Freq:");
        });
        let carrier_slider = egui::Slider::new(&mut self.wf_carrier_freq, 1000.0..=20000.0)
            .logarithmic(true)
            .suffix(" Hz");
        if ui.add(carrier_slider).changed() {
            params_changed = true;
        }

        // Sample rate
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label("Sample Rate:");
        });
        egui::ComboBox::from_id_salt("ook_sample_rate")
            .selected_text(format!("{} Hz", self.wf_sample_rate as u32))
            .show_ui(ui, |ui| {
                for rate in [8000.0, 16000.0, 44100.0, 48000.0, 96000.0] {
                    if ui.selectable_value(&mut self.wf_sample_rate, rate, format!("{} Hz", rate as u32)).changed() {
                        params_changed = true;
                    }
                }
            });

        ui.add_space(12.0);
        ui.separator();

        // Channel Model
        ui.heading("Channel Model");
        ui.add_space(8.0);

        ui.horizontal(|ui| {
            ui.label("SNR (dB):");
        });
        let snr_slider = egui::Slider::new(&mut self.wf_snr_db, -10.0..=40.0);
        if ui.add(snr_slider).changed() {
            params_changed = true;
        }

        ui.horizontal(|ui| {
            ui.label("Model:");
            egui::ComboBox::from_id_salt("ook_channel_model")
                .selected_text(format!("{:?}", self.wf_channel_model))
                .show_ui(ui, |ui| {
                    if ui.selectable_value(&mut self.wf_channel_model, ChannelModel::Ideal, "Ideal").changed() {
                        params_changed = true;
                    }
                    if ui.selectable_value(&mut self.wf_channel_model, ChannelModel::Awgn, "AWGN").changed() {
                        params_changed = true;
                    }
                });
        });

        ui.add_space(12.0);
        ui.separator();

        // Test Payload
        ui.heading("Test Payload");
        ui.add_space(8.0);

        ui.label("Bits (0s and 1s):");
        let response = ui.text_edit_singleline(&mut self.wf_test_bits);
        if response.changed() {
            params_changed = true;
        }
        // Sanitize only when focus is lost (to avoid cursor issues)
        if response.lost_focus() {
            let filtered: String = self.wf_test_bits.chars().filter(|c| *c == '0' || *c == '1').collect();
            if filtered != self.wf_test_bits {
                self.wf_test_bits = filtered;
            }
        }

        ui.add_space(12.0);

        // Generate button
        ui.horizontal(|ui| {
            let needs_initial = self.auto_update && self.generated_samples.is_none();
            if ui.button("Generate Signal").clicked() || (self.auto_update && params_changed) || needs_initial {
                self.generate_ook_signal();
            }
            ui.checkbox(&mut self.auto_update, "Auto-update");
        });

        ui.add_space(12.0);
        ui.separator();

        // Signal Info
        ui.heading("Signal Info");
        ui.add_space(8.0);

        if let Some(ref samples) = self.generated_samples {
            let duration_ms = samples.len() as f64 / self.wf_sample_rate * 1000.0;
            let bit_rate = self.wf_symbol_rate; // OOK: 1 bit per symbol
            ui.label(format!("Samples: {}", samples.len()));
            ui.label(format!("Duration: {:.2} ms", duration_ms));
            ui.label(format!("Bit rate: {:.0} bps", bit_rate));
            ui.label(format!("Bits: {}", self.wf_test_bits.len()));

            if let Some(ber) = self.wf_ber {
                ui.label(format!("BER: {:.2}%", ber * 100.0));
            }
        } else {
            ui.label("No signal generated");
        }
    }

    /// Generate OOK signal with channel effects
    fn generate_ook_signal(&mut self) {
        use std::f64::consts::PI;

        // Parse bits
        let bits: Vec<u8> = self.wf_test_bits.chars()
            .filter_map(|c| match c {
                '0' => Some(0),
                '1' => Some(1),
                _ => None,
            })
            .collect();

        if bits.is_empty() {
            return;
        }

        let samples_per_symbol = (self.wf_sample_rate / self.wf_symbol_rate) as usize;
        let mut samples = Vec::with_capacity(bits.len() * samples_per_symbol);

        for bit in &bits {
            for i in 0..samples_per_symbol {
                let t = i as f64 / self.wf_sample_rate;
                let phase = 2.0 * PI * self.wf_carrier_freq * t;

                let amplitude = if *bit == 1 { self.wf_amplitude as f64 } else { 0.0 };
                let sample = IQSample::new(
                    phase.cos() * amplitude,
                    phase.sin() * amplitude,
                );
                samples.push(sample);
            }
        }

        // Apply channel effects
        if self.wf_channel_model == ChannelModel::Awgn {
            self.apply_awgn_noise(&mut samples);
        }

        // Demodulate and calculate BER
        let demod_bits = self.demodulate_ook(&samples, samples_per_symbol);
        let errors: usize = bits.iter().zip(demod_bits.iter())
            .map(|(tx, rx)| if tx != rx { 1 } else { 0 })
            .sum();
        self.wf_ber = Some(errors as f64 / bits.len() as f64);
        self.wf_demod_bits = Some(demod_bits);

        self.generated_samples = Some(samples);
    }

    /// Apply AWGN noise to samples
    fn apply_awgn_noise(&self, samples: &mut [IQSample]) {
        use rand_distr::{Distribution, Normal};

        // Guard against empty samples
        if samples.is_empty() {
            return;
        }

        let snr_linear = 10.0_f64.powf(self.wf_snr_db as f64 / 10.0);
        let signal_power: f64 = samples.iter()
            .map(|s| s.re * s.re + s.im * s.im)
            .sum::<f64>() / samples.len() as f64;

        // Guard against zero or invalid signal power
        if signal_power <= 0.0 || !signal_power.is_finite() {
            return;
        }

        let noise_power = signal_power / snr_linear;
        let noise_std = (noise_power / 2.0).sqrt();

        // Guard against invalid noise_std (must be positive and finite)
        if noise_std <= 0.0 || !noise_std.is_finite() {
            return;
        }

        let mut rng = rand::thread_rng();
        let normal = Normal::new(0.0, noise_std).unwrap();
        for sample in samples.iter_mut() {
            sample.re += normal.sample(&mut rng);
            sample.im += normal.sample(&mut rng);
        }
    }

    /// Demodulate OOK signal
    fn demodulate_ook(&self, samples: &[IQSample], samples_per_symbol: usize) -> Vec<u8> {
        let mut bits = Vec::new();
        let threshold = self.wf_amplitude as f64 / 2.0;

        for chunk in samples.chunks(samples_per_symbol) {
            let avg_power: f64 = chunk.iter()
                .map(|s| (s.re * s.re + s.im * s.im).sqrt())
                .sum::<f64>() / chunk.len() as f64;
            bits.push(if avg_power > threshold { 1 } else { 0 });
        }

        bits
    }

    /// Render AM (Amplitude Modulation) parameters
    fn render_am_params(&mut self, ui: &mut egui::Ui) {
        let am_type = self.selected_waveform.clone();
        let is_4am = am_type == "4-AM";
        ui.heading(format!("{} Parameters", if is_4am { "4-AM (PAM-4)" } else { "AM" }));
        ui.add_space(8.0);

        let mut params_changed = false;

        // Symbol rate
        ui.horizontal(|ui| {
            ui.label("Symbol Rate:");
        });
        let sym_slider = egui::Slider::new(&mut self.wf_symbol_rate, 100.0..=10000.0)
            .logarithmic(true)
            .suffix(" sym/s");
        if ui.add(sym_slider).changed() {
            params_changed = true;
        }

        // Carrier frequency
        ui.horizontal(|ui| {
            ui.label("Carrier Frequency:");
        });
        let carrier_slider = egui::Slider::new(&mut self.wf_carrier_freq, 1000.0..=20000.0)
            .logarithmic(true)
            .suffix(" Hz");
        if ui.add(carrier_slider).changed() {
            params_changed = true;
        }

        // Modulation index
        ui.horizontal(|ui| {
            ui.label("Modulation Index:");
        });
        let mod_slider = egui::Slider::new(&mut self.wf_am_mod_index, 0.1..=1.5)
            .suffix("")
            .fixed_decimals(2);
        if ui.add(mod_slider).changed() {
            params_changed = true;
        }

        // Show modulation depth percentage
        ui.label(format!("  Modulation depth: {:.0}%", self.wf_am_mod_index * 100.0));
        if self.wf_am_mod_index > 1.0 {
            ui.label(egui::RichText::new("  ⚠ Over-modulation (distortion)").color(egui::Color32::YELLOW));
        }

        ui.add_space(4.0);

        // Suppress carrier checkbox
        if ui.checkbox(&mut self.wf_am_suppress_carrier, "Suppress Carrier (DSB-SC)").changed() {
            params_changed = true;
        }

        ui.add_space(12.0);
        ui.separator();

        // Test payload
        ui.heading("Test Payload");
        ui.add_space(8.0);

        ui.label("Bits (0s and 1s):");
        let response = ui.text_edit_singleline(&mut self.wf_test_bits);
        if response.changed() {
            params_changed = true;
        }
        if response.lost_focus() {
            let filtered: String = self.wf_test_bits.chars().filter(|c| *c == '0' || *c == '1').collect();
            if filtered != self.wf_test_bits {
                self.wf_test_bits = filtered;
            }
        }

        ui.add_space(12.0);

        ui.horizontal(|ui| {
            let needs_initial = self.auto_update && self.generated_samples.is_none();
            if ui.button("Generate Signal").clicked() || (self.auto_update && params_changed) || needs_initial {
                self.generate_am_signal();
            }
            ui.checkbox(&mut self.auto_update, "Auto-update");
        });

        ui.add_space(12.0);
        ui.separator();

        // Signal Info
        ui.heading("Signal Info");
        ui.add_space(8.0);

        if let Some(ref samples) = self.generated_samples {
            let bits_per_symbol = if is_4am { 2 } else { 1 };
            let bit_rate = self.wf_symbol_rate * bits_per_symbol as f64;
            let duration_ms = samples.len() as f64 / self.wf_sample_rate * 1000.0;
            ui.label(format!("Samples: {}", samples.len()));
            ui.label(format!("Duration: {:.2} ms", duration_ms));
            ui.label(format!("Bit Rate: {:.0} bps", bit_rate));
            ui.label(format!("Mode: {}", if self.wf_am_suppress_carrier { "DSB-SC" } else { "DSB-AM" }));

            // BER if demodulated
            if let Some(ber) = self.wf_ber {
                ui.label(format!("BER: {:.2}%", ber * 100.0));
            }
        } else {
            ui.label("No signal generated yet.");
        }
    }

    /// Generate AM signal
    fn generate_am_signal(&mut self) {
        use std::f64::consts::PI;

        let bits: Vec<u8> = self.wf_test_bits.chars()
            .filter_map(|c| match c {
                '0' => Some(0),
                '1' => Some(1),
                _ => None,
            })
            .collect();

        if bits.is_empty() {
            return;
        }

        let is_4am = self.selected_waveform == "4-AM";
        let bits_per_symbol = if is_4am { 2 } else { 1 };
        let num_levels = if is_4am { 4 } else { 2 };
        let samples_per_symbol = (self.wf_sample_rate / self.wf_symbol_rate) as usize;
        let omega = 2.0 * PI * self.wf_carrier_freq / self.wf_sample_rate;

        let mut samples = Vec::new();
        let mut phase = 0.0;

        // Process bits into symbols
        let mut padded_bits = bits.clone();
        while padded_bits.len() % bits_per_symbol != 0 {
            padded_bits.push(0);
        }

        for chunk in padded_bits.chunks(bits_per_symbol) {
            let mut symbol = 0u8;
            for (i, &bit) in chunk.iter().enumerate() {
                symbol |= bit << (bits_per_symbol - 1 - i);
            }

            // Calculate envelope based on symbol
            let envelope = if self.wf_am_suppress_carrier {
                // DSB-SC: amplitude varies from -m to +m
                let normalized = if num_levels == 2 {
                    if symbol == 0 { -1.0 } else { 1.0 }
                } else {
                    let norm = symbol as f64 / (num_levels - 1) as f64;
                    2.0 * norm - 1.0
                };
                normalized * self.wf_am_mod_index
            } else {
                // Standard AM: DC offset + modulation
                if num_levels == 2 {
                    if symbol == 0 {
                        1.0 - self.wf_am_mod_index
                    } else {
                        1.0 + self.wf_am_mod_index
                    }
                } else {
                    let normalized = symbol as f64 / (num_levels - 1) as f64;
                    let modulated = 2.0 * normalized - 1.0;
                    1.0 + self.wf_am_mod_index * modulated
                }
            };

            // Generate samples for this symbol
            for n in 0..samples_per_symbol {
                let p = phase + omega * n as f64;
                let amp = self.wf_amplitude as f64 * envelope;
                samples.push(IQSample::new(amp * p.cos(), amp * p.sin()));
            }
            phase += omega * samples_per_symbol as f64;
        }

        // Demodulate for BER calculation
        let demod_bits = self.demodulate_am(&samples, samples_per_symbol, num_levels);
        let original_bits: Vec<u8> = padded_bits.iter().cloned().collect();
        let errors: usize = demod_bits.iter()
            .zip(original_bits.iter())
            .map(|(a, b)| if a != b { 1 } else { 0 })
            .sum();
        let ber = if !original_bits.is_empty() {
            errors as f64 / original_bits.len() as f64
        } else {
            0.0
        };

        self.generated_samples = Some(samples);
        self.wf_demod_bits = Some(demod_bits);
        self.wf_ber = Some(ber);
    }

    /// Demodulate AM signal
    fn demodulate_am(&self, samples: &[IQSample], samples_per_symbol: usize, num_levels: usize) -> Vec<u8> {
        let mut bits = Vec::new();
        let bits_per_symbol = (num_levels as f64).log2() as usize;

        // Calculate expected amplitude levels
        let expected_levels: Vec<f64> = (0..num_levels)
            .map(|i| {
                if self.wf_am_suppress_carrier {
                    let normalized = i as f64 / (num_levels - 1) as f64;
                    self.wf_amplitude as f64 * self.wf_am_mod_index * (2.0 * normalized - 1.0)
                } else {
                    let envelope = if num_levels == 2 {
                        if i == 0 { 1.0 - self.wf_am_mod_index } else { 1.0 + self.wf_am_mod_index }
                    } else {
                        let normalized = i as f64 / (num_levels - 1) as f64;
                        let modulated = 2.0 * normalized - 1.0;
                        1.0 + self.wf_am_mod_index * modulated
                    };
                    self.wf_amplitude as f64 * envelope
                }
            })
            .collect();

        for chunk in samples.chunks(samples_per_symbol) {
            // RMS amplitude (envelope detection)
            let power: f64 = chunk.iter().map(|s| s.norm_sqr()).sum::<f64>() / chunk.len() as f64;
            let envelope = power.sqrt();

            // Find closest level
            let mut best_symbol = 0u8;
            let mut best_error = f64::MAX;
            for (i, &level) in expected_levels.iter().enumerate() {
                let error = (envelope - level.abs()).abs();
                if error < best_error {
                    best_error = error;
                    best_symbol = i as u8;
                }
            }

            // Convert symbol to bits
            for i in (0..bits_per_symbol).rev() {
                bits.push((best_symbol >> i) & 1);
            }
        }

        bits
    }

    /// Render FM (Frequency Modulation) parameters
    fn render_fm_params(&mut self, ui: &mut egui::Ui) {
        let fm_type = self.selected_waveform.clone();
        let is_4fm = fm_type == "4-FM";
        ui.heading(format!("{} Parameters", if is_4fm { "4-FM" } else { "FM" }));
        ui.add_space(8.0);

        let mut params_changed = false;

        // Symbol rate
        ui.horizontal(|ui| {
            ui.label("Symbol Rate:");
        });
        let sym_slider = egui::Slider::new(&mut self.wf_symbol_rate, 100.0..=10000.0)
            .logarithmic(true)
            .suffix(" sym/s");
        if ui.add(sym_slider).changed() {
            params_changed = true;
        }

        // Carrier frequency
        ui.horizontal(|ui| {
            ui.label("Carrier Frequency:");
        });
        let carrier_slider = egui::Slider::new(&mut self.wf_carrier_freq, 1000.0..=20000.0)
            .logarithmic(true)
            .suffix(" Hz");
        if ui.add(carrier_slider).changed() {
            params_changed = true;
        }

        // Frequency deviation
        ui.horizontal(|ui| {
            ui.label("Frequency Deviation:");
        });
        let dev_slider = egui::Slider::new(&mut self.wf_fsk_deviation, 100.0..=5000.0)
            .suffix(" Hz");
        if ui.add(dev_slider).changed() {
            params_changed = true;
        }

        // Show modulation index (beta)
        let beta = self.wf_fsk_deviation / self.wf_symbol_rate;
        ui.label(format!("  Modulation index (β): {:.2}", beta));
        let fm_mode = if beta < 1.0 { "Narrowband FM (NBFM)" } else { "Wideband FM (WBFM)" };
        ui.label(format!("  Mode: {}", fm_mode));

        // Carson's bandwidth
        let carson_bw = 2.0 * (self.wf_fsk_deviation + self.wf_symbol_rate);
        ui.label(format!("  Carson's BW: {:.0} Hz", carson_bw));

        ui.add_space(12.0);
        ui.separator();

        // Test payload
        ui.heading("Test Payload");
        ui.add_space(8.0);

        ui.label("Bits (0s and 1s):");
        let response = ui.text_edit_singleline(&mut self.wf_test_bits);
        if response.changed() {
            params_changed = true;
        }
        if response.lost_focus() {
            let filtered: String = self.wf_test_bits.chars().filter(|c| *c == '0' || *c == '1').collect();
            if filtered != self.wf_test_bits {
                self.wf_test_bits = filtered;
            }
        }

        ui.add_space(12.0);

        ui.horizontal(|ui| {
            let needs_initial = self.auto_update && self.generated_samples.is_none();
            if ui.button("Generate Signal").clicked() || (self.auto_update && params_changed) || needs_initial {
                self.generate_fm_signal();
            }
            ui.checkbox(&mut self.auto_update, "Auto-update");
        });

        ui.add_space(12.0);
        ui.separator();

        // Signal Info
        ui.heading("Signal Info");
        ui.add_space(8.0);

        if let Some(ref samples) = self.generated_samples {
            let bits_per_symbol = if is_4fm { 2 } else { 1 };
            let bit_rate = self.wf_symbol_rate * bits_per_symbol as f64;
            let duration_ms = samples.len() as f64 / self.wf_sample_rate * 1000.0;
            ui.label(format!("Samples: {}", samples.len()));
            ui.label(format!("Duration: {:.2} ms", duration_ms));
            ui.label(format!("Bit Rate: {:.0} bps", bit_rate));

            // BER if demodulated
            if let Some(ber) = self.wf_ber {
                ui.label(format!("BER: {:.2}%", ber * 100.0));
            }
        } else {
            ui.label("No signal generated yet.");
        }
    }

    /// Generate FM signal
    fn generate_fm_signal(&mut self) {
        use std::f64::consts::PI;

        let bits: Vec<u8> = self.wf_test_bits.chars()
            .filter_map(|c| match c {
                '0' => Some(0),
                '1' => Some(1),
                _ => None,
            })
            .collect();

        if bits.is_empty() {
            return;
        }

        let is_4fm = self.selected_waveform == "4-FM";
        let bits_per_symbol = if is_4fm { 2 } else { 1 };
        let num_levels = if is_4fm { 4 } else { 2 };
        let samples_per_symbol = (self.wf_sample_rate / self.wf_symbol_rate) as usize;

        let mut samples = Vec::new();
        let mut phase = 0.0;

        // Process bits into symbols
        let mut padded_bits = bits.clone();
        while padded_bits.len() % bits_per_symbol != 0 {
            padded_bits.push(0);
        }

        for chunk in padded_bits.chunks(bits_per_symbol) {
            let mut symbol = 0u8;
            for (i, &bit) in chunk.iter().enumerate() {
                symbol |= bit << (bits_per_symbol - 1 - i);
            }

            // Calculate frequency offset for this symbol
            let freq_offset = if num_levels == 2 {
                if symbol == 0 { -self.wf_fsk_deviation } else { self.wf_fsk_deviation }
            } else {
                let normalized = symbol as f64 / (num_levels - 1) as f64;
                let scaled = 2.0 * normalized - 1.0;
                scaled * self.wf_fsk_deviation
            };

            let total_freq = self.wf_carrier_freq + freq_offset;
            let omega = 2.0 * PI * total_freq / self.wf_sample_rate;

            // Generate samples for this symbol (constant envelope)
            for n in 0..samples_per_symbol {
                let p = phase + omega * n as f64;
                samples.push(IQSample::new(
                    self.wf_amplitude as f64 * p.cos(),
                    self.wf_amplitude as f64 * p.sin(),
                ));
            }
            phase += omega * samples_per_symbol as f64;
        }

        // Demodulate for BER calculation
        let demod_bits = self.demodulate_fm(&samples, samples_per_symbol, num_levels);
        let original_bits: Vec<u8> = padded_bits.iter().cloned().collect();
        let errors: usize = demod_bits.iter()
            .zip(original_bits.iter())
            .map(|(a, b)| if a != b { 1 } else { 0 })
            .sum();
        let ber = if !original_bits.is_empty() {
            errors as f64 / original_bits.len() as f64
        } else {
            0.0
        };

        self.generated_samples = Some(samples);
        self.wf_demod_bits = Some(demod_bits);
        self.wf_ber = Some(ber);
    }

    /// Demodulate FM signal
    fn demodulate_fm(&self, samples: &[IQSample], samples_per_symbol: usize, num_levels: usize) -> Vec<u8> {
        use std::f64::consts::PI;

        let mut bits = Vec::new();
        let bits_per_symbol = (num_levels as f64).log2() as usize;

        for chunk in samples.chunks(samples_per_symbol) {
            if chunk.len() < 2 {
                continue;
            }

            // FM demodulation using phase differentiation
            let mut freq_estimates = Vec::new();
            for i in 1..chunk.len() {
                let phase_diff = (chunk[i] * chunk[i - 1].conj()).arg();
                let inst_freq = phase_diff * self.wf_sample_rate / (2.0 * PI);
                freq_estimates.push(inst_freq);
            }

            let avg_freq: f64 = freq_estimates.iter().sum::<f64>() / freq_estimates.len() as f64;
            let freq_offset = avg_freq - self.wf_carrier_freq;

            // Find closest frequency level
            let mut best_symbol = 0u8;
            let mut best_error = f64::MAX;

            for sym in 0..num_levels as u8 {
                let expected_offset = if num_levels == 2 {
                    if sym == 0 { -self.wf_fsk_deviation } else { self.wf_fsk_deviation }
                } else {
                    let normalized = sym as f64 / (num_levels - 1) as f64;
                    let scaled = 2.0 * normalized - 1.0;
                    scaled * self.wf_fsk_deviation
                };
                let error = (freq_offset - expected_offset).abs();
                if error < best_error {
                    best_error = error;
                    best_symbol = sym;
                }
            }

            // Convert symbol to bits
            for i in (0..bits_per_symbol).rev() {
                bits.push((best_symbol >> i) & 1);
            }
        }

        bits
    }

    /// Render PPM (Pulse Position Modulation) parameters
    fn render_ppm_params(&mut self, ui: &mut egui::Ui) {
        let is_adsb = self.selected_waveform == "ADS-B";
        ui.heading(if is_adsb { "ADS-B Parameters" } else { "PPM Parameters" });
        ui.add_space(8.0);

        let mut params_changed = false;

        if is_adsb {
            ui.label("ADS-B Mode S PPM encoding:");
            ui.label("  • 1090 MHz carrier (simulated at baseband)");
            ui.label("  • 1 Mbps data rate");
            ui.label("  • Manchester-like chip encoding");
            ui.label("  • 112-bit extended squitter messages");
        } else {
            // Symbol rate for standard PPM
            ui.horizontal(|ui| {
                ui.label("Symbol Rate:");
            });
            let sym_slider = egui::Slider::new(&mut self.wf_symbol_rate, 100.0..=100000.0)
                .logarithmic(true)
                .suffix(" sym/s");
            if ui.add(sym_slider).changed() {
                params_changed = true;
            }

            let sps = (self.wf_sample_rate / self.wf_symbol_rate) as usize;
            ui.label(format!("  Samples/symbol: {}", sps));
        }

        ui.add_space(12.0);
        ui.separator();

        // Test payload
        ui.heading("Test Payload");
        ui.add_space(8.0);

        ui.label("Bits (0s and 1s):");
        let response = ui.text_edit_singleline(&mut self.wf_test_bits);
        if response.changed() {
            params_changed = true;
        }
        if response.lost_focus() {
            let filtered: String = self.wf_test_bits.chars().filter(|c| *c == '0' || *c == '1').collect();
            if filtered != self.wf_test_bits {
                self.wf_test_bits = filtered;
            }
        }

        ui.add_space(12.0);

        ui.horizontal(|ui| {
            let needs_initial = self.auto_update && self.generated_samples.is_none();
            if ui.button("Generate Signal").clicked() || (self.auto_update && params_changed) || needs_initial {
                self.generate_ppm_signal();
            }
            ui.checkbox(&mut self.auto_update, "Auto-update");
        });

        ui.add_space(12.0);
        ui.separator();

        // Signal Info
        ui.heading("Signal Info");
        ui.add_space(8.0);

        if let Some(ref samples) = self.generated_samples {
            let duration_ms = samples.len() as f64 / self.wf_sample_rate * 1000.0;
            ui.label(format!("Samples: {}", samples.len()));
            ui.label(format!("Duration: {:.2} ms", duration_ms));
            if is_adsb {
                ui.label("Data Rate: 1 Mbps");
                let num_bits = self.wf_test_bits.len();
                ui.label(format!("Message: {} bits (+ 8-bit preamble)", num_bits));
            } else {
                ui.label(format!("Symbol Rate: {:.0} sym/s", self.wf_symbol_rate));
            }

            if let Some(ber) = self.wf_ber {
                ui.label(format!("BER: {:.2}%", ber * 100.0));
            }
        } else {
            ui.label("No signal generated yet.");
        }
    }

    /// Generate PPM signal
    fn generate_ppm_signal(&mut self) {
        let bits: Vec<u8> = self.wf_test_bits.chars()
            .filter_map(|c| match c {
                '0' => Some(0),
                '1' => Some(1),
                _ => None,
            })
            .collect();

        if bits.is_empty() {
            return;
        }

        let is_adsb = self.selected_waveform == "ADS-B";

        // Use the Waveform trait's modulate method
        if let Some(wf) = WaveformFactory::create(
            if is_adsb { "ADS-B" } else { "PPM" },
            self.wf_sample_rate
        ) {
            let samples = wf.modulate(&bits);
            let result = wf.demodulate(&samples);

            // Calculate BER
            let errors: usize = result.bits.iter()
                .zip(bits.iter())
                .map(|(a, b)| if a != b { 1 } else { 0 })
                .sum();
            let ber = if !bits.is_empty() {
                errors as f64 / bits.len() as f64
            } else {
                0.0
            };

            self.generated_samples = Some(samples);
            self.wf_demod_bits = Some(result.bits);
            self.wf_ber = Some(ber);
        }
    }

    /// Render OFDM parameters
    fn render_ofdm_params(&mut self, ui: &mut egui::Ui) {
        ui.heading("OFDM Parameters");
        ui.add_space(8.0);

        let mut params_changed = false;

        // FFT Size
        ui.horizontal(|ui| {
            ui.label("FFT Size:");
        });
        egui::ComboBox::from_id_salt("ofdm_fft_size")
            .selected_text(format!("{}", self.wf_ofdm_fft_size))
            .show_ui(ui, |ui| {
                for size in [32, 64, 128, 256, 512, 1024, 2048] {
                    if ui.selectable_value(&mut self.wf_ofdm_fft_size, size, format!("{}", size)).changed() {
                        // Adjust data subcarriers if needed
                        if self.wf_ofdm_data_subcarriers >= self.wf_ofdm_fft_size {
                            self.wf_ofdm_data_subcarriers = self.wf_ofdm_fft_size * 3 / 4;
                        }
                        params_changed = true;
                    }
                }
            });

        // Data subcarriers
        let max_data = self.wf_ofdm_fft_size - 1;
        ui.horizontal(|ui| {
            ui.label("Data Subcarriers:");
        });
        let data_slider = egui::Slider::new(&mut self.wf_ofdm_data_subcarriers, 4..=max_data);
        if ui.add(data_slider).changed() {
            params_changed = true;
        }

        // Cyclic prefix ratio
        ui.horizontal(|ui| {
            ui.label("Cyclic Prefix:");
        });
        egui::ComboBox::from_id_salt("ofdm_cp_ratio")
            .selected_text(format!("1/{}", (1.0 / self.wf_ofdm_cp_ratio).round() as usize))
            .show_ui(ui, |ui| {
                for (ratio, label) in [(0.0625, "1/16"), (0.125, "1/8"), (0.25, "1/4"), (0.5, "1/2")] {
                    if ui.selectable_value(&mut self.wf_ofdm_cp_ratio, ratio, label).changed() {
                        params_changed = true;
                    }
                }
            });

        // Subcarrier modulation
        ui.horizontal(|ui| {
            ui.label("Subcarrier Mod:");
        });
        let mod_names = ["BPSK", "QPSK", "16-QAM", "64-QAM"];
        egui::ComboBox::from_id_salt("ofdm_subcarrier_mod")
            .selected_text(mod_names[self.wf_ofdm_subcarrier_mod])
            .show_ui(ui, |ui| {
                for (i, name) in mod_names.iter().enumerate() {
                    if ui.selectable_value(&mut self.wf_ofdm_subcarrier_mod, i, *name).changed() {
                        params_changed = true;
                    }
                }
            });

        ui.add_space(8.0);

        // Computed parameters
        let bits_per_sc = [1, 2, 4, 6][self.wf_ofdm_subcarrier_mod];
        let bits_per_symbol = self.wf_ofdm_data_subcarriers * bits_per_sc;
        let cp_len = (self.wf_ofdm_fft_size as f64 * self.wf_ofdm_cp_ratio) as usize;
        let symbol_samples = self.wf_ofdm_fft_size + cp_len;
        let symbol_duration_us = symbol_samples as f64 / self.wf_sample_rate * 1_000_000.0;
        let subcarrier_spacing = self.wf_sample_rate / self.wf_ofdm_fft_size as f64;
        let data_rate = bits_per_symbol as f64 / (symbol_samples as f64 / self.wf_sample_rate);

        ui.label(format!("  Bits/OFDM symbol: {}", bits_per_symbol));
        ui.label(format!("  CP length: {} samples", cp_len));
        ui.label(format!("  Symbol duration: {:.1} µs", symbol_duration_us));
        ui.label(format!("  Subcarrier spacing: {:.1} Hz", subcarrier_spacing));
        ui.label(format!("  Data rate: {:.1} kbps", data_rate / 1000.0));

        ui.add_space(12.0);
        ui.separator();

        // Test payload
        ui.heading("Test Payload");
        ui.add_space(8.0);

        ui.label("Bits (0s and 1s):");
        let response = ui.text_edit_singleline(&mut self.wf_test_bits);
        if response.changed() {
            params_changed = true;
        }
        if response.lost_focus() {
            let filtered: String = self.wf_test_bits.chars().filter(|c| *c == '0' || *c == '1').collect();
            if filtered != self.wf_test_bits {
                self.wf_test_bits = filtered;
            }
        }

        ui.add_space(12.0);

        ui.horizontal(|ui| {
            let needs_initial = self.auto_update && self.generated_samples.is_none();
            if ui.button("Generate Signal").clicked() || (self.auto_update && params_changed) || needs_initial {
                self.generate_ofdm_signal();
            }
            ui.checkbox(&mut self.auto_update, "Auto-update");
        });

        ui.add_space(12.0);
        ui.separator();

        // Signal Info
        ui.heading("Signal Info");
        ui.add_space(8.0);

        if let Some(ref samples) = self.generated_samples {
            let duration_ms = samples.len() as f64 / self.wf_sample_rate * 1000.0;
            let num_ofdm_symbols = samples.len() / symbol_samples;
            ui.label(format!("Samples: {}", samples.len()));
            ui.label(format!("Duration: {:.2} ms", duration_ms));
            ui.label(format!("OFDM symbols: {}", num_ofdm_symbols));

            if let Some(ber) = self.wf_ber {
                ui.label(format!("BER: {:.2}%", ber * 100.0));
            }
        } else {
            ui.label("No signal generated yet.");
        }
    }

    /// Generate OFDM signal
    fn generate_ofdm_signal(&mut self) {
        use r4w_core::waveform::ofdm::{OFDM, SubcarrierModulation};
        use r4w_core::waveform::{CommonParams, Waveform};

        // Convert test bits string to packed bytes (consistent with other waveforms)
        let bits: Vec<u8> = self.wf_test_bits.chars()
            .filter_map(|c| match c {
                '0' => Some(0u8),
                '1' => Some(1u8),
                _ => None,
            })
            .collect();

        if bits.is_empty() {
            return;
        }

        // Pack individual bits into bytes
        let data: Vec<u8> = bits.chunks(8)
            .map(|chunk| {
                chunk.iter()
                    .enumerate()
                    .fold(0u8, |acc, (i, &bit)| acc | (bit << (7 - i)))
            })
            .collect();

        let common = CommonParams {
            sample_rate: self.wf_sample_rate,
            carrier_freq: 0.0,
            amplitude: self.wf_amplitude as f64,
        };

        let subcarrier_mod = match self.wf_ofdm_subcarrier_mod {
            0 => SubcarrierModulation::Bpsk,
            1 => SubcarrierModulation::Qpsk,
            2 => SubcarrierModulation::Qam16,
            _ => SubcarrierModulation::Qam64,
        };

        let ofdm = OFDM::new(
            common,
            self.wf_ofdm_fft_size,
            self.wf_ofdm_data_subcarriers,
            self.wf_ofdm_cp_ratio,
            subcarrier_mod,
        );

        // Modulate and demodulate with packed bytes
        let samples = ofdm.modulate(&data);
        let result = ofdm.demodulate(&samples);

        // Calculate BER (compare packed bytes)
        let compare_len = data.len().min(result.bits.len());
        let mut bit_errors = 0;
        let mut total_bits = 0;
        for i in 0..compare_len {
            let diff = data[i] ^ result.bits[i];
            bit_errors += diff.count_ones() as usize;
            total_bits += 8;
        }
        let ber = if total_bits > 0 {
            bit_errors as f64 / total_bits as f64
        } else {
            0.0
        };

        self.generated_samples = Some(samples);
        self.wf_demod_bits = Some(result.bits);
        self.wf_ber = Some(ber);
    }

    /// Render DSSS (Direct Sequence Spread Spectrum) parameters
    fn render_dsss_params(&mut self, ui: &mut egui::Ui) {
        ui.heading("DSSS Parameters");
        ui.add_space(8.0);

        let mut params_changed = false;

        // PN Degree (determines chips per symbol)
        let degree_options = [
            (5_u8, "31 chips (15 dB)"),
            (6, "63 chips (18 dB)"),
            (7, "127 chips (21 dB)"),
            (8, "255 chips (24 dB)"),
            (9, "511 chips (27 dB)"),
            (10, "1023 chips (30 dB)"),
        ];

        ui.horizontal(|ui| {
            ui.label("Processing Gain:");
        });

        let current_label = degree_options
            .iter()
            .find(|(d, _)| *d == self.wf_dsss_pn_degree)
            .map(|(_, l)| *l)
            .unwrap_or("127 chips (21 dB)");

        egui::ComboBox::from_id_salt("dsss_pn_degree")
            .selected_text(current_label)
            .show_ui(ui, |ui| {
                for (degree, label) in degree_options {
                    if ui.selectable_value(&mut self.wf_dsss_pn_degree, degree, label).changed() {
                        params_changed = true;
                    }
                }
            });

        // Modulation type
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label("Modulation:");
        });

        let mod_names = ["BPSK (1 bit/symbol)", "QPSK (2 bits/symbol)"];
        egui::ComboBox::from_id_salt("dsss_modulation")
            .selected_text(mod_names[self.wf_dsss_modulation])
            .show_ui(ui, |ui| {
                for (i, name) in mod_names.iter().enumerate() {
                    if ui.selectable_value(&mut self.wf_dsss_modulation, i, *name).changed() {
                        params_changed = true;
                    }
                }
            });

        // Samples per chip
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label("Samples per chip:");
        });
        let spc_slider = egui::Slider::new(&mut self.wf_dsss_samples_per_chip, 2..=8);
        if ui.add(spc_slider).changed() {
            params_changed = true;
        }

        // Computed values
        ui.add_space(12.0);
        ui.separator();
        ui.add_space(8.0);

        let chips_per_symbol = (1 << self.wf_dsss_pn_degree) - 1;
        let processing_gain_db = 10.0 * (chips_per_symbol as f64).log10();
        let chip_rate = self.wf_sample_rate / self.wf_dsss_samples_per_chip as f64;
        let symbol_rate = chip_rate / chips_per_symbol as f64;
        let bits_per_symbol = if self.wf_dsss_modulation == 0 { 1 } else { 2 };
        let data_rate = symbol_rate * bits_per_symbol as f64;
        let spread_bandwidth = chip_rate;

        ui.label(format!("Chips per symbol: {}", chips_per_symbol));
        ui.label(format!("Processing gain: {:.1} dB", processing_gain_db));
        ui.label(format!("Chip rate: {:.1} chips/s", chip_rate));
        ui.label(format!("Symbol rate: {:.2} sym/s", symbol_rate));
        ui.label(format!("Data rate: {:.2} bps", data_rate));
        ui.label(format!("Spread bandwidth: {:.1} Hz", spread_bandwidth));

        // LPI indicator
        ui.add_space(8.0);
        if processing_gain_db >= 20.0 {
            ui.colored_label(egui::Color32::GREEN, "Good LPD/LPI capability");
        } else if processing_gain_db >= 15.0 {
            ui.colored_label(egui::Color32::YELLOW, "Moderate LPD/LPI");
        } else {
            ui.colored_label(egui::Color32::LIGHT_GRAY, "Basic spread spectrum");
        }

        // Channel Model
        ui.add_space(16.0);
        ui.heading("Channel Model");
        ui.add_space(8.0);

        ui.horizontal(|ui| {
            ui.label("SNR (dB):");
        });
        let snr_slider = egui::Slider::new(&mut self.wf_snr_db, -10.0..=40.0);
        if ui.add(snr_slider).changed() {
            params_changed = true;
        }

        // Test Payload
        ui.add_space(16.0);
        ui.heading("Test Payload");
        ui.add_space(8.0);

        ui.horizontal(|ui| {
            ui.label("Test Bits:");
        });
        let response = ui.text_edit_singleline(&mut self.wf_test_bits);
        if response.lost_focus() {
            self.wf_test_bits = self.wf_test_bits.chars().filter(|c| *c == '0' || *c == '1').collect();
        }

        ui.add_space(8.0);

        let needs_initial = self.auto_update && self.generated_samples.is_none();
        if ui.button("Generate Signal").clicked() || (self.auto_update && params_changed) || needs_initial {
            self.generate_dsss_signal();
        }

        ui.checkbox(&mut self.auto_update, "Auto-update");

        // Signal Info
        if let Some(ref samples) = self.generated_samples {
            ui.add_space(16.0);
            ui.heading("Signal Info");
            ui.add_space(8.0);

            ui.label(format!("Samples: {}", samples.len()));
            let duration_ms = samples.len() as f64 / self.wf_sample_rate * 1000.0;
            ui.label(format!("Duration: {:.2} ms", duration_ms));

            let bits: Vec<u8> = self.wf_test_bits.chars()
                .filter_map(|c| match c { '0' => Some(0), '1' => Some(1), _ => None })
                .collect();
            ui.label(format!("Bits: {}", bits.len()));

            if let Some(ber) = self.wf_ber {
                let ber_color = if ber == 0.0 {
                    egui::Color32::GREEN
                } else if ber < 0.01 {
                    egui::Color32::YELLOW
                } else {
                    egui::Color32::RED
                };
                ui.colored_label(ber_color, format!("BER: {:.2}%", ber * 100.0));
            }
        }
    }

    /// Generate DSSS signal from current parameters
    fn generate_dsss_signal(&mut self) {
        use r4w_core::waveform::dsss::{DSSS, DsssConfig, DsssModulation, PnSequenceType};
        use r4w_core::waveform::{CommonParams, Waveform};

        let bits: Vec<u8> = self.wf_test_bits.chars()
            .filter_map(|c| match c {
                '0' => Some(0),
                '1' => Some(1),
                _ => None,
            })
            .collect();

        if bits.is_empty() {
            return;
        }

        let common = CommonParams {
            sample_rate: self.wf_sample_rate,
            carrier_freq: 0.0,
            amplitude: self.wf_amplitude as f64,
        };

        let modulation = if self.wf_dsss_modulation == 0 {
            DsssModulation::Bpsk
        } else {
            DsssModulation::Qpsk
        };

        let config = DsssConfig {
            pn_type: PnSequenceType::Gold,
            pn_degree: self.wf_dsss_pn_degree,
            code_index: 2,
            modulation,
            samples_per_chip: self.wf_dsss_samples_per_chip,
        };

        let dsss = DSSS::new(common, config);
        let mut samples = dsss.modulate(&bits);

        // Apply noise
        self.apply_awgn_noise(&mut samples);

        // Demodulate
        let result = dsss.demodulate(&samples);

        // Calculate BER
        let compare_len = bits.len().min(result.bits.len());
        let errors: usize = result.bits[..compare_len].iter()
            .zip(bits[..compare_len].iter())
            .map(|(a, b)| if a != b { 1 } else { 0 })
            .sum();
        let ber = if compare_len > 0 {
            errors as f64 / compare_len as f64
        } else {
            0.0
        };

        self.generated_samples = Some(samples);
        self.wf_demod_bits = Some(result.bits);
        self.wf_ber = Some(ber);
    }

    /// Render FHSS (Frequency Hopping Spread Spectrum) parameters
    fn render_fhss_params(&mut self, ui: &mut egui::Ui) {
        ui.heading("FHSS Parameters");
        ui.add_space(8.0);

        let mut params_changed = false;

        // Number of channels
        ui.horizontal(|ui| {
            ui.label("Hop Channels:");
        });
        let ch_slider = egui::Slider::new(&mut self.wf_fhss_num_channels, 10..=200);
        if ui.add(ch_slider).changed() {
            params_changed = true;
        }

        // Channel spacing
        ui.horizontal(|ui| {
            ui.label("Channel Spacing:");
        });
        let spacing_slider = egui::Slider::new(&mut self.wf_fhss_channel_spacing, 1000.0..=100000.0)
            .logarithmic(true)
            .suffix(" Hz");
        if ui.add(spacing_slider).changed() {
            params_changed = true;
        }

        // Hop rate
        ui.horizontal(|ui| {
            ui.label("Hop Rate:");
        });
        let hop_slider = egui::Slider::new(&mut self.wf_fhss_hop_rate, 10.0..=1000.0)
            .logarithmic(true)
            .suffix(" hops/s");
        if ui.add(hop_slider).changed() {
            params_changed = true;
        }

        // Symbols per hop
        ui.horizontal(|ui| {
            ui.label("Symbols per Hop:");
        });
        let sph_slider = egui::Slider::new(&mut self.wf_fhss_symbols_per_hop, 1..=50);
        if ui.add(sph_slider).changed() {
            params_changed = true;
        }

        // Hop pattern
        ui.add_space(8.0);
        let pattern_names = ["Pseudo-Random", "Sequential"];
        ui.horizontal(|ui| {
            ui.label("Hop Pattern:");
        });
        egui::ComboBox::from_id_salt("fhss_pattern")
            .selected_text(pattern_names[self.wf_fhss_pattern])
            .show_ui(ui, |ui| {
                for (i, name) in pattern_names.iter().enumerate() {
                    if ui.selectable_value(&mut self.wf_fhss_pattern, i, *name).changed() {
                        params_changed = true;
                    }
                }
            });

        // Modulation at each hop
        ui.add_space(8.0);
        let mod_names = ["BFSK", "BPSK", "QPSK"];
        ui.horizontal(|ui| {
            ui.label("Hop Modulation:");
        });
        egui::ComboBox::from_id_salt("fhss_modulation")
            .selected_text(mod_names[self.wf_fhss_modulation])
            .show_ui(ui, |ui| {
                for (i, name) in mod_names.iter().enumerate() {
                    if ui.selectable_value(&mut self.wf_fhss_modulation, i, *name).changed() {
                        params_changed = true;
                    }
                }
            });

        // Computed values
        ui.add_space(12.0);
        ui.separator();
        ui.add_space(8.0);

        let total_bandwidth = self.wf_fhss_num_channels as f64 * self.wf_fhss_channel_spacing;
        let hop_bandwidth = self.wf_fhss_channel_spacing; // Approximate
        let processing_gain_db = 10.0 * (total_bandwidth / hop_bandwidth).log10();
        let dwell_time_ms = 1000.0 / self.wf_fhss_hop_rate;
        let bits_per_symbol = match self.wf_fhss_modulation {
            2 => 2, // QPSK
            _ => 1, // BFSK, BPSK
        };
        // Symbol rate is symbols_per_hop * hop_rate
        let effective_symbol_rate = self.wf_fhss_symbols_per_hop as f64 * self.wf_fhss_hop_rate;
        let data_rate = effective_symbol_rate * bits_per_symbol as f64;

        ui.label(format!("Total bandwidth: {:.1} kHz", total_bandwidth / 1000.0));
        ui.label(format!("Processing gain: {:.1} dB", processing_gain_db));
        ui.label(format!("Dwell time: {:.2} ms", dwell_time_ms));
        ui.label(format!("Effective symbol rate: {:.1} sym/s", effective_symbol_rate));
        ui.label(format!("Data rate: {:.1} bps", data_rate));

        // Hopping mode indicator
        ui.add_space(8.0);
        if self.wf_fhss_symbols_per_hop == 1 {
            ui.colored_label(egui::Color32::GREEN, "Fast hopping (1 symbol/hop)");
        } else if self.wf_fhss_symbols_per_hop <= 5 {
            ui.colored_label(egui::Color32::YELLOW, "Moderate hopping");
        } else {
            ui.colored_label(egui::Color32::LIGHT_GRAY, "Slow hopping");
        }

        // Channel Model
        ui.add_space(16.0);
        ui.heading("Channel Model");
        ui.add_space(8.0);

        ui.horizontal(|ui| {
            ui.label("SNR (dB):");
        });
        let snr_slider = egui::Slider::new(&mut self.wf_snr_db, -10.0..=40.0);
        if ui.add(snr_slider).changed() {
            params_changed = true;
        }

        // Test Payload
        ui.add_space(16.0);
        ui.heading("Test Payload");
        ui.add_space(8.0);

        ui.horizontal(|ui| {
            ui.label("Test Bits:");
        });
        let response = ui.text_edit_singleline(&mut self.wf_test_bits);
        if response.lost_focus() {
            self.wf_test_bits = self.wf_test_bits.chars().filter(|c| *c == '0' || *c == '1').collect();
        }

        ui.add_space(8.0);

        let needs_initial = self.auto_update && self.generated_samples.is_none();
        if ui.button("Generate Signal").clicked() || (self.auto_update && params_changed) || needs_initial {
            self.generate_fhss_signal();
        }

        ui.checkbox(&mut self.auto_update, "Auto-update");

        // Signal Info
        if let Some(ref samples) = self.generated_samples {
            ui.add_space(16.0);
            ui.heading("Signal Info");
            ui.add_space(8.0);

            ui.label(format!("Samples: {}", samples.len()));
            let duration_ms = samples.len() as f64 / self.wf_sample_rate * 1000.0;
            ui.label(format!("Duration: {:.2} ms", duration_ms));

            let bits: Vec<u8> = self.wf_test_bits.chars()
                .filter_map(|c| match c { '0' => Some(0), '1' => Some(1), _ => None })
                .collect();
            ui.label(format!("Bits: {}", bits.len()));

            // Calculate number of hops
            let bits_per_hop = self.wf_fhss_symbols_per_hop * bits_per_symbol;
            let num_hops = (bits.len() + bits_per_hop - 1) / bits_per_hop;
            ui.label(format!("Hops used: {}", num_hops));
        }
    }

    /// Generate FHSS signal from current parameters
    fn generate_fhss_signal(&mut self) {
        use r4w_core::waveform::fhss::{FHSS, FhssConfig, HopModulation, HopPattern};
        use r4w_core::waveform::{CommonParams, Waveform};

        let bits: Vec<u8> = self.wf_test_bits.chars()
            .filter_map(|c| match c {
                '0' => Some(0),
                '1' => Some(1),
                _ => None,
            })
            .collect();

        if bits.is_empty() {
            return;
        }

        let common = CommonParams {
            sample_rate: self.wf_sample_rate,
            carrier_freq: 0.0,
            amplitude: self.wf_amplitude as f64,
        };

        let hop_pattern = if self.wf_fhss_pattern == 0 {
            HopPattern::PseudoRandom
        } else {
            HopPattern::Sequential
        };

        let modulation = match self.wf_fhss_modulation {
            1 => HopModulation::Bpsk,
            2 => HopModulation::Qpsk,
            _ => HopModulation::Bfsk { deviation: 5000.0 },
        };

        // Calculate symbol rate based on hop rate and symbols per hop
        let symbol_rate = self.wf_fhss_symbols_per_hop as f64 * self.wf_fhss_hop_rate;

        let config = FhssConfig {
            num_channels: self.wf_fhss_num_channels,
            channel_spacing: self.wf_fhss_channel_spacing,
            hop_rate: self.wf_fhss_hop_rate,
            symbols_per_hop: self.wf_fhss_symbols_per_hop,
            symbol_rate,
            hop_pattern,
            modulation,
            seed: 0x12345,
        };

        let fhss = FHSS::new(common, config);
        let mut samples = fhss.modulate(&bits);

        // Apply noise
        self.apply_awgn_noise(&mut samples);

        // Note: FHSS demodulation requires sync, so we don't calculate BER here
        self.generated_samples = Some(samples);
        self.wf_demod_bits = None;
        self.wf_ber = None;
    }

    /// Render Zigbee (IEEE 802.15.4) parameters
    fn render_zigbee_params(&mut self, ui: &mut egui::Ui) {
        ui.heading("Zigbee / 802.15.4 Parameters");
        ui.add_space(8.0);

        let mut params_changed = false;

        // Samples per chip
        ui.horizontal(|ui| {
            ui.label("Samples per chip:");
        });
        let spc_slider = egui::Slider::new(&mut self.wf_zigbee_samples_per_chip, 1..=8);
        if ui.add(spc_slider).changed() {
            params_changed = true;
        }

        // Half-sine shaping
        ui.add_space(8.0);
        if ui.checkbox(&mut self.wf_zigbee_half_sine, "Half-sine pulse shaping").changed() {
            params_changed = true;
        }
        ui.label("(Standard 802.15.4 uses half-sine for spectral efficiency)");

        // Fixed parameters info
        ui.add_space(12.0);
        ui.separator();
        ui.add_space(8.0);

        ui.label("802.15.4 Standard (2.4 GHz):");
        ui.label("  O-QPSK modulation with DSSS");
        ui.label("  32-chip spreading sequences");
        ui.label("  4 bits per symbol (0-15)");

        // Computed values
        ui.add_space(12.0);
        let chip_rate = self.wf_sample_rate / self.wf_zigbee_samples_per_chip as f64;
        let symbol_rate = chip_rate / 32.0;
        let data_rate = symbol_rate * 4.0;
        let processing_gain_db = 10.0 * 32.0_f64.log10();

        ui.label(format!("Chip rate: {:.1} kchips/s", chip_rate / 1000.0));
        ui.label(format!("Symbol rate: {:.2} ksym/s", symbol_rate / 1000.0));
        ui.label(format!("Data rate: {:.2} kbps", data_rate / 1000.0));
        ui.label(format!("Processing gain: {:.1} dB", processing_gain_db));

        // Channel Model
        ui.add_space(16.0);
        ui.heading("Channel Model");
        ui.add_space(8.0);

        ui.horizontal(|ui| {
            ui.label("SNR (dB):");
        });
        let snr_slider = egui::Slider::new(&mut self.wf_snr_db, -10.0..=40.0);
        if ui.add(snr_slider).changed() {
            params_changed = true;
        }

        // Test Payload
        ui.add_space(16.0);
        ui.heading("Test Payload");
        ui.add_space(8.0);

        ui.horizontal(|ui| {
            ui.label("Test Bits:");
        });
        let response = ui.text_edit_singleline(&mut self.wf_test_bits);
        if response.lost_focus() {
            self.wf_test_bits = self.wf_test_bits.chars().filter(|c| *c == '0' || *c == '1').collect();
        }

        ui.add_space(8.0);

        let needs_initial = self.auto_update && self.generated_samples.is_none();
        if ui.button("Generate Signal").clicked() || (self.auto_update && params_changed) || needs_initial {
            self.generate_zigbee_signal();
        }

        ui.checkbox(&mut self.auto_update, "Auto-update");

        // Signal Info
        if let Some(ref samples) = self.generated_samples {
            ui.add_space(16.0);
            ui.heading("Signal Info");
            ui.add_space(8.0);

            ui.label(format!("Samples: {}", samples.len()));
            let duration_ms = samples.len() as f64 / self.wf_sample_rate * 1000.0;
            ui.label(format!("Duration: {:.2} ms", duration_ms));

            let bits: Vec<u8> = self.wf_test_bits.chars()
                .filter_map(|c| match c { '0' => Some(0), '1' => Some(1), _ => None })
                .collect();
            let num_symbols = (bits.len() + 3) / 4;
            ui.label(format!("Bits: {} ({} symbols)", bits.len(), num_symbols));

            if let Some(ber) = self.wf_ber {
                let ber_color = if ber == 0.0 {
                    egui::Color32::GREEN
                } else if ber < 0.01 {
                    egui::Color32::YELLOW
                } else {
                    egui::Color32::RED
                };
                ui.colored_label(ber_color, format!("BER: {:.2}%", ber * 100.0));
            }
        }
    }

    /// Generate Zigbee signal from current parameters
    fn generate_zigbee_signal(&mut self) {
        use r4w_core::waveform::zigbee::Zigbee;
        use r4w_core::waveform::{CommonParams, Waveform};

        let bits: Vec<u8> = self.wf_test_bits.chars()
            .filter_map(|c| match c {
                '0' => Some(0),
                '1' => Some(1),
                _ => None,
            })
            .collect();

        if bits.is_empty() {
            return;
        }

        let common = CommonParams {
            sample_rate: self.wf_sample_rate,
            carrier_freq: 0.0,
            amplitude: self.wf_amplitude as f64,
        };

        let zigbee = Zigbee::new(common, self.wf_zigbee_samples_per_chip, self.wf_zigbee_half_sine);
        let mut samples = zigbee.modulate(&bits);

        // Apply noise
        self.apply_awgn_noise(&mut samples);

        // Demodulate
        let result = zigbee.demodulate(&samples);

        // Calculate BER
        let compare_len = bits.len().min(result.bits.len());
        let errors: usize = result.bits[..compare_len].iter()
            .zip(bits[..compare_len].iter())
            .map(|(a, b)| if a != b { 1 } else { 0 })
            .sum();
        let ber = if compare_len > 0 {
            errors as f64 / compare_len as f64
        } else {
            0.0
        };

        self.generated_samples = Some(samples);
        self.wf_demod_bits = Some(result.bits);
        self.wf_ber = Some(ber);
    }

    /// Render UWB (Ultra-Wideband Impulse Radio) parameters
    fn render_uwb_params(&mut self, ui: &mut egui::Ui) {
        ui.heading("UWB Impulse Radio Parameters");
        ui.add_space(8.0);

        let mut params_changed = false;

        // Pulse shape
        let shape_names = ["Gaussian Monocycle", "Gaussian Doublet", "Raised Cosine", "Rectangular"];
        ui.horizontal(|ui| {
            ui.label("Pulse Shape:");
        });
        egui::ComboBox::from_id_salt("uwb_pulse_shape")
            .selected_text(shape_names[self.wf_uwb_pulse_shape])
            .show_ui(ui, |ui| {
                for (i, name) in shape_names.iter().enumerate() {
                    if ui.selectable_value(&mut self.wf_uwb_pulse_shape, i, *name).changed() {
                        params_changed = true;
                    }
                }
            });

        // Modulation
        ui.add_space(8.0);
        let mod_names = ["OOK (On-Off)", "BPSK (Polarity)", "PPM (Position)"];
        ui.horizontal(|ui| {
            ui.label("Modulation:");
        });
        egui::ComboBox::from_id_salt("uwb_modulation")
            .selected_text(mod_names[self.wf_uwb_modulation])
            .show_ui(ui, |ui| {
                for (i, name) in mod_names.iter().enumerate() {
                    if ui.selectable_value(&mut self.wf_uwb_modulation, i, *name).changed() {
                        params_changed = true;
                    }
                }
            });

        // Pulse duration
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label("Pulse Duration:");
        });
        let dur_slider = egui::Slider::new(&mut self.wf_uwb_pulse_duration_ns, 0.5..=10.0)
            .suffix(" ns");
        if ui.add(dur_slider).changed() {
            params_changed = true;
        }

        // Pulse interval
        ui.horizontal(|ui| {
            ui.label("Pulse Interval:");
        });
        let int_slider = egui::Slider::new(&mut self.wf_uwb_pulse_interval_ns, 10.0..=1000.0)
            .logarithmic(true)
            .suffix(" ns");
        if ui.add(int_slider).changed() {
            params_changed = true;
        }

        // Pulses per bit
        ui.horizontal(|ui| {
            ui.label("Pulses per Bit:");
        });
        let ppb_slider = egui::Slider::new(&mut self.wf_uwb_pulses_per_bit, 1..=32);
        if ui.add(ppb_slider).changed() {
            params_changed = true;
        }

        // Computed values
        ui.add_space(12.0);
        ui.separator();
        ui.add_space(8.0);

        let pulse_duration_s = self.wf_uwb_pulse_duration_ns * 1e-9;
        let pulse_interval_s = self.wf_uwb_pulse_interval_ns * 1e-9;
        let bandwidth_hz = 1.0 / pulse_duration_s;
        let prf = 1.0 / pulse_interval_s;
        let data_rate = prf / self.wf_uwb_pulses_per_bit as f64;
        let processing_gain_db = 10.0 * (bandwidth_hz / data_rate).log10();

        ui.label(format!("Bandwidth: {:.0} MHz", bandwidth_hz / 1e6));
        ui.label(format!("PRF: {:.2} MHz", prf / 1e6));
        ui.label(format!("Data rate: {:.2} Mbps", data_rate / 1e6));
        ui.label(format!("Processing gain: {:.1} dB", processing_gain_db));

        // UWB indicators
        ui.add_space(8.0);
        if bandwidth_hz >= 500e6 {
            ui.colored_label(egui::Color32::GREEN, "Meets FCC UWB definition (>500 MHz)");
        } else {
            ui.colored_label(egui::Color32::YELLOW, format!("Narrower than FCC UWB ({:.0} MHz < 500 MHz)", bandwidth_hz / 1e6));
        }

        if processing_gain_db >= 20.0 {
            ui.colored_label(egui::Color32::GREEN, "Excellent LPD/LPI capability");
        }

        // Channel Model
        ui.add_space(16.0);
        ui.heading("Channel Model");
        ui.add_space(8.0);

        ui.horizontal(|ui| {
            ui.label("SNR (dB):");
        });
        let snr_slider = egui::Slider::new(&mut self.wf_snr_db, -10.0..=40.0);
        if ui.add(snr_slider).changed() {
            params_changed = true;
        }

        // Test Payload
        ui.add_space(16.0);
        ui.heading("Test Payload");
        ui.add_space(8.0);

        ui.horizontal(|ui| {
            ui.label("Test Bits:");
        });
        let response = ui.text_edit_singleline(&mut self.wf_test_bits);
        if response.lost_focus() {
            self.wf_test_bits = self.wf_test_bits.chars().filter(|c| *c == '0' || *c == '1').collect();
        }

        ui.add_space(8.0);

        let needs_initial = self.auto_update && self.generated_samples.is_none();
        if ui.button("Generate Signal").clicked() || (self.auto_update && params_changed) || needs_initial {
            self.generate_uwb_signal();
        }

        ui.checkbox(&mut self.auto_update, "Auto-update");

        // Signal Info
        if let Some(ref samples) = self.generated_samples {
            ui.add_space(16.0);
            ui.heading("Signal Info");
            ui.add_space(8.0);

            ui.label(format!("Samples: {}", samples.len()));
            let duration_ms = samples.len() as f64 / self.wf_sample_rate * 1000.0;
            ui.label(format!("Duration: {:.2} ms", duration_ms));

            let bits: Vec<u8> = self.wf_test_bits.chars()
                .filter_map(|c| match c { '0' => Some(0), '1' => Some(1), _ => None })
                .collect();
            ui.label(format!("Bits: {}", bits.len()));
            ui.label(format!("Pulses: {}", bits.len() * self.wf_uwb_pulses_per_bit));

            if let Some(ber) = self.wf_ber {
                let ber_color = if ber == 0.0 {
                    egui::Color32::GREEN
                } else if ber < 0.01 {
                    egui::Color32::YELLOW
                } else {
                    egui::Color32::RED
                };
                ui.colored_label(ber_color, format!("BER: {:.2}%", ber * 100.0));
            }
        }
    }

    /// Generate UWB signal from current parameters
    fn generate_uwb_signal(&mut self) {
        use r4w_core::waveform::uwb::{UwbIr, UwbConfig, PulseShape, UwbModulation};
        use r4w_core::waveform::{CommonParams, Waveform};

        let bits: Vec<u8> = self.wf_test_bits.chars()
            .filter_map(|c| match c {
                '0' => Some(0),
                '1' => Some(1),
                _ => None,
            })
            .collect();

        if bits.is_empty() {
            return;
        }

        let common = CommonParams {
            sample_rate: self.wf_sample_rate,
            carrier_freq: 0.0,
            amplitude: self.wf_amplitude as f64,
        };

        let pulse_shape = match self.wf_uwb_pulse_shape {
            0 => PulseShape::GaussianMonocycle,
            1 => PulseShape::GaussianDoublet,
            2 => PulseShape::RaisedCosine,
            _ => PulseShape::Rectangular,
        };

        let modulation = match self.wf_uwb_modulation {
            0 => UwbModulation::Ook,
            2 => UwbModulation::Ppm { shift_samples: 10 },
            _ => UwbModulation::Bpsk,
        };

        let config = UwbConfig {
            pulse_shape,
            modulation,
            pulse_duration_s: self.wf_uwb_pulse_duration_ns * 1e-9,
            pulse_interval_s: self.wf_uwb_pulse_interval_ns * 1e-9,
            pulses_per_bit: self.wf_uwb_pulses_per_bit,
        };

        let uwb = UwbIr::new(common, config);
        let mut samples = uwb.modulate(&bits);

        // Apply noise
        self.apply_awgn_noise(&mut samples);

        // Demodulate
        let result = uwb.demodulate(&samples);

        // Calculate BER
        let compare_len = bits.len().min(result.bits.len());
        let errors: usize = result.bits[..compare_len].iter()
            .zip(bits[..compare_len].iter())
            .map(|(a, b)| if a != b { 1 } else { 0 })
            .sum();
        let ber = if compare_len > 0 {
            errors as f64 / compare_len as f64
        } else {
            0.0
        };

        self.generated_samples = Some(samples);
        self.wf_demod_bits = Some(result.bits);
        self.wf_ber = Some(ber);
    }

    /// Render FMCW Radar parameters
    fn render_fmcw_params(&mut self, ui: &mut egui::Ui) {
        ui.heading("FMCW Radar Parameters");
        ui.add_space(8.0);

        ui.colored_label(egui::Color32::LIGHT_BLUE, "Note: FMCW is a radar waveform, not communications");
        ui.add_space(8.0);

        let mut params_changed = false;

        // Chirp bandwidth
        ui.horizontal(|ui| {
            ui.label("Chirp Bandwidth:");
        });
        let bw_slider = egui::Slider::new(&mut self.wf_fmcw_bandwidth_mhz, 10.0..=1000.0)
            .logarithmic(true)
            .suffix(" MHz");
        if ui.add(bw_slider).changed() {
            params_changed = true;
        }

        // Chirp duration
        ui.horizontal(|ui| {
            ui.label("Chirp Duration:");
        });
        let dur_slider = egui::Slider::new(&mut self.wf_fmcw_chirp_duration_us, 10.0..=200.0)
            .suffix(" µs");
        if ui.add(dur_slider).changed() {
            params_changed = true;
        }

        // Number of chirps
        ui.horizontal(|ui| {
            ui.label("Number of Chirps:");
        });
        let chirps_slider = egui::Slider::new(&mut self.wf_fmcw_num_chirps, 1..=16);
        if ui.add(chirps_slider).changed() {
            params_changed = true;
        }

        // Chirp direction
        ui.add_space(8.0);
        let dir_names = ["Up", "Down", "Triangle", "Sawtooth"];
        ui.horizontal(|ui| {
            ui.label("Chirp Pattern:");
        });
        egui::ComboBox::from_id_salt("fmcw_chirp_direction")
            .selected_text(dir_names[self.wf_fmcw_chirp_direction])
            .show_ui(ui, |ui| {
                for (i, name) in dir_names.iter().enumerate() {
                    if ui.selectable_value(&mut self.wf_fmcw_chirp_direction, i, *name).changed() {
                        params_changed = true;
                    }
                }
            });

        // Computed radar parameters
        ui.add_space(12.0);
        ui.separator();
        ui.add_space(8.0);
        ui.heading("Radar Performance");

        let bandwidth_hz = self.wf_fmcw_bandwidth_mhz * 1e6;
        let chirp_duration_s = self.wf_fmcw_chirp_duration_us * 1e-6;
        let chirp_rate = bandwidth_hz / chirp_duration_s;
        let range_resolution = 299_792_458.0 / (2.0 * bandwidth_hz);

        // Assume 77 GHz carrier for velocity calculations
        let carrier_freq = 77e9;
        let wavelength = 299_792_458.0 / carrier_freq;
        let frame_time = self.wf_fmcw_num_chirps as f64 * chirp_duration_s;
        let velocity_resolution = wavelength / (2.0 * frame_time);
        let max_velocity = wavelength / (4.0 * chirp_duration_s);

        ui.label(format!("Chirp rate: {:.2} MHz/µs", chirp_rate / 1e12));
        ui.label(format!("Range resolution: {:.2} m", range_resolution));
        ui.label(format!("Velocity resolution: {:.3} m/s", velocity_resolution));
        ui.label(format!("Max velocity: {:.1} m/s", max_velocity));

        // Application hints
        ui.add_space(8.0);
        if bandwidth_hz >= 1e9 {
            ui.colored_label(egui::Color32::GREEN, "Wide bandwidth: Fine range resolution");
        } else if bandwidth_hz >= 500e6 {
            ui.colored_label(egui::Color32::YELLOW, "Medium bandwidth");
        } else {
            ui.colored_label(egui::Color32::LIGHT_GRAY, "Narrow bandwidth: Coarse range resolution");
        }

        if self.wf_fmcw_num_chirps >= 8 {
            ui.colored_label(egui::Color32::GREEN, "Multiple chirps: Good velocity estimation");
        }

        // Generate button
        ui.add_space(16.0);

        let needs_initial = self.auto_update && self.generated_samples.is_none();
        if ui.button("Generate Radar Waveform").clicked() || (self.auto_update && params_changed) || needs_initial {
            self.generate_fmcw_signal();
        }

        ui.checkbox(&mut self.auto_update, "Auto-update");

        // Signal Info
        if let Some(ref samples) = self.generated_samples {
            ui.add_space(16.0);
            ui.heading("Signal Info");
            ui.add_space(8.0);

            ui.label(format!("Samples: {}", samples.len()));
            let duration_ms = samples.len() as f64 / self.wf_sample_rate * 1000.0;
            ui.label(format!("Duration: {:.2} ms", duration_ms));
            ui.label(format!("Chirps: {}", self.wf_fmcw_num_chirps));

            let samples_per_chirp = (chirp_duration_s * self.wf_sample_rate) as usize;
            ui.label(format!("Samples per chirp: {}", samples_per_chirp));
        }
    }

    /// Generate FMCW radar signal from current parameters
    fn generate_fmcw_signal(&mut self) {
        use r4w_core::waveform::fmcw::{Fmcw, FmcwConfig, ChirpDirection};
        use r4w_core::waveform::{CommonParams, Waveform};

        let common = CommonParams {
            sample_rate: self.wf_sample_rate,
            carrier_freq: 0.0,
            amplitude: self.wf_amplitude as f64,
        };

        let chirp_direction = match self.wf_fmcw_chirp_direction {
            0 => ChirpDirection::Up,
            1 => ChirpDirection::Down,
            2 => ChirpDirection::Triangle,
            _ => ChirpDirection::Sawtooth,
        };

        let config = FmcwConfig {
            bandwidth_hz: self.wf_fmcw_bandwidth_mhz * 1e6,
            chirp_duration_s: self.wf_fmcw_chirp_duration_us * 1e-6,
            num_chirps: self.wf_fmcw_num_chirps,
            idle_time_s: 0.0,
            chirp_direction,
            start_freq_offset_hz: -self.wf_fmcw_bandwidth_mhz * 0.5e6,
            use_window: false,
        };

        let fmcw = Fmcw::new(common, config);

        // For FMCW, we just generate chirps (no data encoding)
        let samples = fmcw.modulate(&[]);

        self.generated_samples = Some(samples);
        // FMCW is radar, not communications - no BER
        self.wf_demod_bits = None;
        self.wf_ber = None;
    }

    /// Render FSK parameters
    fn render_fsk_params(&mut self, ui: &mut egui::Ui) {
        let fsk_type = self.selected_waveform.clone();
        ui.heading(format!("{} Parameters", fsk_type));
        ui.add_space(8.0);

        let mut params_changed = false;

        // Symbol rate
        ui.horizontal(|ui| {
            ui.label("Symbol Rate:");
        });
        let sym_slider = egui::Slider::new(&mut self.wf_symbol_rate, 100.0..=10000.0)
            .logarithmic(true)
            .suffix(" sym/s");
        if ui.add(sym_slider).changed() {
            params_changed = true;
        }

        // Deviation
        ui.horizontal(|ui| {
            ui.label("Deviation:");
        });
        let dev_slider = egui::Slider::new(&mut self.wf_fsk_deviation, 100.0..=5000.0)
            .suffix(" Hz");
        if ui.add(dev_slider).changed() {
            params_changed = true;
        }

        // Modulation index (computed)
        let mod_index = 2.0 * self.wf_fsk_deviation / self.wf_symbol_rate;
        ui.add_space(4.0);
        ui.label(format!("Modulation index: {:.2}", mod_index));
        if mod_index >= 0.5 {
            ui.label("(Wideband FM)");
        } else {
            ui.label("(Narrowband FM)");
        }

        // Sample rate
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label("Sample Rate:");
        });
        egui::ComboBox::from_id_salt("fsk_sample_rate")
            .selected_text(format!("{} Hz", self.wf_sample_rate as u32))
            .show_ui(ui, |ui| {
                for rate in [8000.0, 16000.0, 44100.0, 48000.0, 96000.0] {
                    if ui.selectable_value(&mut self.wf_sample_rate, rate, format!("{} Hz", rate as u32)).changed() {
                        params_changed = true;
                    }
                }
            });

        ui.add_space(12.0);
        ui.separator();

        // Channel Model
        ui.heading("Channel Model");
        ui.add_space(8.0);

        ui.horizontal(|ui| {
            ui.label("SNR (dB):");
        });
        let snr_slider = egui::Slider::new(&mut self.wf_snr_db, -10.0..=40.0);
        if ui.add(snr_slider).changed() {
            params_changed = true;
        }

        ui.horizontal(|ui| {
            ui.label("Model:");
            egui::ComboBox::from_id_salt("fsk_channel_model")
                .selected_text(format!("{:?}", self.wf_channel_model))
                .show_ui(ui, |ui| {
                    if ui.selectable_value(&mut self.wf_channel_model, ChannelModel::Ideal, "Ideal").changed() {
                        params_changed = true;
                    }
                    if ui.selectable_value(&mut self.wf_channel_model, ChannelModel::Awgn, "AWGN").changed() {
                        params_changed = true;
                    }
                });
        });

        ui.add_space(12.0);
        ui.separator();

        // Test Payload
        ui.heading("Test Payload");
        ui.add_space(8.0);

        ui.label("Bits (0s and 1s):");
        let response = ui.text_edit_singleline(&mut self.wf_test_bits);
        if response.changed() {
            params_changed = true;
        }
        if response.lost_focus() {
            let filtered: String = self.wf_test_bits.chars().filter(|c| *c == '0' || *c == '1').collect();
            if filtered != self.wf_test_bits {
                self.wf_test_bits = filtered;
            }
        }

        ui.add_space(12.0);

        ui.horizontal(|ui| {
            let needs_initial = self.auto_update && self.generated_samples.is_none();
            if ui.button("Generate Signal").clicked() || (self.auto_update && params_changed) || needs_initial {
                self.generate_fsk_signal();
            }
            ui.checkbox(&mut self.auto_update, "Auto-update");
        });

        ui.add_space(12.0);
        ui.separator();

        // Signal Info
        ui.heading("Signal Info");
        ui.add_space(8.0);

        if let Some(ref samples) = self.generated_samples {
            let bits_per_symbol = if self.selected_waveform == "4-FSK" { 2 } else { 1 };
            let bit_rate = self.wf_symbol_rate * bits_per_symbol as f64;
            let duration_ms = samples.len() as f64 / self.wf_sample_rate * 1000.0;
            ui.label(format!("Samples: {}", samples.len()));
            ui.label(format!("Duration: {:.2} ms", duration_ms));
            ui.label(format!("Bit rate: {:.0} bps", bit_rate));
            ui.label(format!("Bits: {}", self.wf_test_bits.len()));

            if let Some(ber) = self.wf_ber {
                ui.label(format!("BER: {:.2}%", ber * 100.0));
            }
        } else {
            ui.label("No signal generated");
        }
    }

    /// Generate FSK signal
    fn generate_fsk_signal(&mut self) {
        use std::f64::consts::PI;

        let bits: Vec<u8> = self.wf_test_bits.chars()
            .filter_map(|c| match c {
                '0' => Some(0),
                '1' => Some(1),
                _ => None,
            })
            .collect();

        if bits.is_empty() {
            return;
        }

        let samples_per_symbol = (self.wf_sample_rate / self.wf_symbol_rate) as usize;
        let mut samples = Vec::with_capacity(bits.len() * samples_per_symbol);
        let mut phase = 0.0_f64;

        for bit in &bits {
            let freq = if *bit == 1 {
                self.wf_carrier_freq + self.wf_fsk_deviation
            } else {
                self.wf_carrier_freq - self.wf_fsk_deviation
            };

            for _ in 0..samples_per_symbol {
                let sample = IQSample::new(
                    phase.cos() * self.wf_amplitude as f64,
                    phase.sin() * self.wf_amplitude as f64,
                );
                samples.push(sample);
                phase += 2.0 * PI * freq / self.wf_sample_rate;
            }
        }

        // Normalize phase
        while phase > 2.0 * PI {
            phase -= 2.0 * PI;
        }

        // Apply channel effects
        if self.wf_channel_model == ChannelModel::Awgn {
            self.apply_awgn_noise(&mut samples);
        }

        // Demodulate and calculate BER
        let demod_bits = self.demodulate_fsk(&samples, samples_per_symbol);
        let errors: usize = bits.iter().zip(demod_bits.iter())
            .map(|(tx, rx)| if tx != rx { 1 } else { 0 })
            .sum();
        self.wf_ber = Some(errors as f64 / bits.len() as f64);
        self.wf_demod_bits = Some(demod_bits);

        self.generated_samples = Some(samples);
    }

    /// Demodulate FSK signal
    fn demodulate_fsk(&self, samples: &[IQSample], samples_per_symbol: usize) -> Vec<u8> {
        use std::f64::consts::PI;

        let mut bits = Vec::new();
        let freq_high = self.wf_carrier_freq + self.wf_fsk_deviation;
        let freq_low = self.wf_carrier_freq - self.wf_fsk_deviation;

        for chunk in samples.chunks(samples_per_symbol) {
            // Correlate with high and low frequency
            let mut corr_high = 0.0_f64;
            let mut corr_low = 0.0_f64;

            for (i, sample) in chunk.iter().enumerate() {
                let t = i as f64 / self.wf_sample_rate;
                let phase_high = 2.0 * PI * freq_high * t;
                let phase_low = 2.0 * PI * freq_low * t;

                corr_high += sample.re as f64 * phase_high.cos() + sample.im as f64 * phase_high.sin();
                corr_low += sample.re as f64 * phase_low.cos() + sample.im as f64 * phase_low.sin();
            }

            bits.push(if corr_high > corr_low { 1 } else { 0 });
        }

        bits
    }

    /// Render PSK parameters
    fn render_psk_params(&mut self, ui: &mut egui::Ui) {
        let psk_type = self.selected_waveform.clone();
        ui.heading(format!("{} Parameters", psk_type));
        ui.add_space(8.0);

        let mut params_changed = false;

        // Symbol rate
        ui.horizontal(|ui| {
            ui.label("Symbol Rate:");
        });
        let sym_slider = egui::Slider::new(&mut self.wf_symbol_rate, 100.0..=10000.0)
            .logarithmic(true)
            .suffix(" sym/s");
        if ui.add(sym_slider).changed() {
            params_changed = true;
        }

        // Sample rate
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label("Sample Rate:");
        });
        egui::ComboBox::from_id_salt("psk_sample_rate")
            .selected_text(format!("{} Hz", self.wf_sample_rate as u32))
            .show_ui(ui, |ui| {
                for rate in [8000.0, 16000.0, 44100.0, 48000.0, 96000.0] {
                    if ui.selectable_value(&mut self.wf_sample_rate, rate, format!("{} Hz", rate as u32)).changed() {
                        params_changed = true;
                    }
                }
            });

        // Show bits per symbol
        let bits_per_symbol = match psk_type.as_str() {
            "BPSK" => 1,
            "QPSK" => 2,
            "8-PSK" => 3,
            _ => 1,
        };
        ui.add_space(4.0);
        ui.label(format!("Bits per symbol: {}", bits_per_symbol));

        ui.add_space(12.0);
        ui.separator();

        // Channel Model
        ui.heading("Channel Model");
        ui.add_space(8.0);

        ui.horizontal(|ui| {
            ui.label("SNR (dB):");
        });
        let snr_slider = egui::Slider::new(&mut self.wf_snr_db, -10.0..=40.0);
        if ui.add(snr_slider).changed() {
            params_changed = true;
        }

        ui.horizontal(|ui| {
            ui.label("Model:");
            egui::ComboBox::from_id_salt("psk_channel_model")
                .selected_text(format!("{:?}", self.wf_channel_model))
                .show_ui(ui, |ui| {
                    if ui.selectable_value(&mut self.wf_channel_model, ChannelModel::Ideal, "Ideal").changed() {
                        params_changed = true;
                    }
                    if ui.selectable_value(&mut self.wf_channel_model, ChannelModel::Awgn, "AWGN").changed() {
                        params_changed = true;
                    }
                });
        });

        ui.add_space(12.0);
        ui.separator();

        // Test Payload
        ui.heading("Test Payload");
        ui.add_space(8.0);

        ui.label("Bits (0s and 1s):");
        let response = ui.text_edit_singleline(&mut self.wf_test_bits);
        if response.changed() {
            params_changed = true;
        }
        if response.lost_focus() {
            let filtered: String = self.wf_test_bits.chars().filter(|c| *c == '0' || *c == '1').collect();
            if filtered != self.wf_test_bits {
                self.wf_test_bits = filtered;
            }
        }

        ui.add_space(12.0);

        ui.horizontal(|ui| {
            let needs_initial = self.auto_update && self.generated_samples.is_none();
            if ui.button("Generate Signal").clicked() || (self.auto_update && params_changed) || needs_initial {
                self.generate_psk_signal();
            }
            ui.checkbox(&mut self.auto_update, "Auto-update");
        });

        ui.add_space(12.0);
        ui.separator();

        // Signal Info
        ui.heading("Signal Info");
        ui.add_space(8.0);

        if let Some(ref samples) = self.generated_samples {
            let bit_rate = self.wf_symbol_rate * bits_per_symbol as f64;
            let duration_ms = samples.len() as f64 / self.wf_sample_rate * 1000.0;
            ui.label(format!("Samples: {}", samples.len()));
            ui.label(format!("Duration: {:.2} ms", duration_ms));
            ui.label(format!("Bit rate: {:.0} bps", bit_rate));
            ui.label(format!("Symbols: {}", self.wf_test_bits.len() / bits_per_symbol));

            if let Some(ber) = self.wf_ber {
                ui.label(format!("BER: {:.2}%", ber * 100.0));
            }
        } else {
            ui.label("No signal generated");
        }
    }

    /// Generate PSK signal
    fn generate_psk_signal(&mut self) {
        use std::f64::consts::PI;

        let bits: Vec<u8> = self.wf_test_bits.chars()
            .filter_map(|c| match c {
                '0' => Some(0),
                '1' => Some(1),
                _ => None,
            })
            .collect();

        if bits.is_empty() {
            return;
        }

        let bits_per_symbol = match self.selected_waveform.as_str() {
            "BPSK" => 1,
            "QPSK" => 2,
            "8-PSK" => 3,
            _ => 1,
        };

        let num_phases = 1 << bits_per_symbol;
        let samples_per_symbol = (self.wf_sample_rate / self.wf_symbol_rate) as usize;

        // Pad bits to multiple of bits_per_symbol
        let mut padded_bits = bits.clone();
        while padded_bits.len() % bits_per_symbol != 0 {
            padded_bits.push(0);
        }

        let mut samples = Vec::new();

        for chunk in padded_bits.chunks(bits_per_symbol) {
            // Convert bits to symbol index
            let mut symbol_idx = 0u8;
            for (i, bit) in chunk.iter().enumerate() {
                symbol_idx |= bit << (bits_per_symbol - 1 - i);
            }

            // Gray coding for symbol index
            let gray_idx = symbol_idx ^ (symbol_idx >> 1);

            // Calculate phase
            let phase_offset = 2.0 * PI * gray_idx as f64 / num_phases as f64;

            // Generate samples for this symbol
            for i in 0..samples_per_symbol {
                let t = i as f64 / self.wf_sample_rate;
                let phase = 2.0 * PI * self.wf_carrier_freq * t + phase_offset;
                let sample = IQSample::new(
                    phase.cos() * self.wf_amplitude as f64,
                    phase.sin() * self.wf_amplitude as f64,
                );
                samples.push(sample);
            }
        }

        // Apply channel effects
        if self.wf_channel_model == ChannelModel::Awgn {
            self.apply_awgn_noise(&mut samples);
        }

        // Demodulate and calculate BER
        let demod_bits = self.demodulate_psk(&samples, samples_per_symbol, bits_per_symbol);
        let compare_len = bits.len().min(demod_bits.len());
        let errors: usize = bits.iter().take(compare_len).zip(demod_bits.iter())
            .map(|(tx, rx)| if tx != rx { 1 } else { 0 })
            .sum();
        self.wf_ber = Some(errors as f64 / compare_len as f64);
        self.wf_demod_bits = Some(demod_bits);

        self.generated_samples = Some(samples);
    }

    /// Demodulate PSK signal
    fn demodulate_psk(&self, samples: &[IQSample], samples_per_symbol: usize, bits_per_symbol: usize) -> Vec<u8> {
        use std::f64::consts::PI;

        let num_phases = 1 << bits_per_symbol;
        let mut bits = Vec::new();

        for chunk in samples.chunks(samples_per_symbol) {
            // Calculate average I and Q for this symbol
            let avg_i: f64 = chunk.iter().map(|s| s.re).sum::<f64>() / chunk.len() as f64;
            let avg_q: f64 = chunk.iter().map(|s| s.im).sum::<f64>() / chunk.len() as f64;

            // Get phase
            let phase = avg_q.atan2(avg_i);
            let normalized_phase = if phase < 0.0 { phase + 2.0 * PI } else { phase };

            // Quantize to nearest symbol
            let symbol_idx = ((normalized_phase * num_phases as f64 / (2.0 * PI) + 0.5) as usize) % num_phases;

            // Reverse Gray coding
            let mut gray_idx = symbol_idx;
            let mut binary = gray_idx;
            while gray_idx > 0 {
                gray_idx >>= 1;
                binary ^= gray_idx;
            }

            // Extract bits
            for i in (0..bits_per_symbol).rev() {
                bits.push(((binary >> i) & 1) as u8);
            }
        }

        bits
    }

    /// Render QAM parameters
    fn render_qam_params(&mut self, ui: &mut egui::Ui) {
        let qam_type = self.selected_waveform.clone();
        ui.heading(format!("{} Parameters", qam_type));
        ui.add_space(8.0);

        let mut params_changed = false;

        // Symbol rate
        ui.horizontal(|ui| {
            ui.label("Symbol Rate:");
        });
        let sym_slider = egui::Slider::new(&mut self.wf_symbol_rate, 100.0..=10000.0)
            .logarithmic(true)
            .suffix(" sym/s");
        if ui.add(sym_slider).changed() {
            params_changed = true;
        }

        // Sample rate
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label("Sample Rate:");
        });
        egui::ComboBox::from_id_salt("qam_sample_rate")
            .selected_text(format!("{} Hz", self.wf_sample_rate as u32))
            .show_ui(ui, |ui| {
                for rate in [8000.0, 16000.0, 44100.0, 48000.0, 96000.0] {
                    if ui.selectable_value(&mut self.wf_sample_rate, rate, format!("{} Hz", rate as u32)).changed() {
                        params_changed = true;
                    }
                }
            });

        // Show bits per symbol and required SNR
        let (bits_per_symbol, order) = match qam_type.as_str() {
            "16-QAM" => (4, 16),
            "64-QAM" => (6, 64),
            "256-QAM" => (8, 256),
            _ => (4, 16),
        };

        // Approximate required SNR for BER = 10^-6
        let required_snr_db = 10.0 * (order as f64).log10() + 4.0;

        ui.add_space(4.0);
        ui.label(format!("Bits per symbol: {}", bits_per_symbol));
        ui.label(format!("Required SNR: ~{:.0} dB", required_snr_db));

        ui.add_space(12.0);
        ui.separator();

        // Channel Model
        ui.heading("Channel Model");
        ui.add_space(8.0);

        ui.horizontal(|ui| {
            ui.label("SNR (dB):");
        });
        let snr_slider = egui::Slider::new(&mut self.wf_snr_db, -10.0..=40.0);
        if ui.add(snr_slider).changed() {
            params_changed = true;
        }

        // SNR margin indicator
        let margin = self.wf_snr_db as f64 - required_snr_db;
        if margin < 0.0 {
            ui.colored_label(egui::Color32::RED, format!("⚠ {:.1} dB below required", -margin));
        } else {
            ui.colored_label(egui::Color32::GREEN, format!("✓ {:.1} dB margin", margin));
        }

        ui.horizontal(|ui| {
            ui.label("Model:");
            egui::ComboBox::from_id_salt("qam_channel_model")
                .selected_text(format!("{:?}", self.wf_channel_model))
                .show_ui(ui, |ui| {
                    if ui.selectable_value(&mut self.wf_channel_model, ChannelModel::Ideal, "Ideal").changed() {
                        params_changed = true;
                    }
                    if ui.selectable_value(&mut self.wf_channel_model, ChannelModel::Awgn, "AWGN").changed() {
                        params_changed = true;
                    }
                });
        });

        ui.add_space(12.0);
        ui.separator();

        // Test Payload
        ui.heading("Test Payload");
        ui.add_space(8.0);

        ui.label("Bits (0s and 1s):");
        let response = ui.text_edit_singleline(&mut self.wf_test_bits);
        if response.changed() {
            params_changed = true;
        }
        if response.lost_focus() {
            let filtered: String = self.wf_test_bits.chars().filter(|c| *c == '0' || *c == '1').collect();
            if filtered != self.wf_test_bits {
                self.wf_test_bits = filtered;
            }
        }

        ui.add_space(12.0);

        ui.horizontal(|ui| {
            let needs_initial = self.auto_update && self.generated_samples.is_none();
            if ui.button("Generate Signal").clicked() || (self.auto_update && params_changed) || needs_initial {
                self.generate_qam_signal();
            }
            ui.checkbox(&mut self.auto_update, "Auto-update");
        });

        ui.add_space(12.0);
        ui.separator();

        // Signal Info
        ui.heading("Signal Info");
        ui.add_space(8.0);

        if let Some(ref samples) = self.generated_samples {
            let bit_rate = self.wf_symbol_rate * bits_per_symbol as f64;
            let duration_ms = samples.len() as f64 / self.wf_sample_rate * 1000.0;
            ui.label(format!("Samples: {}", samples.len()));
            ui.label(format!("Duration: {:.2} ms", duration_ms));
            ui.label(format!("Bit rate: {:.0} bps", bit_rate));
            ui.label(format!("Symbols: {}", self.wf_test_bits.len() / bits_per_symbol));

            if let Some(ber) = self.wf_ber {
                ui.label(format!("BER: {:.2}%", ber * 100.0));
            }
        } else {
            ui.label("No signal generated");
        }
    }

    /// Generate QAM signal
    fn generate_qam_signal(&mut self) {
        use std::f64::consts::PI;

        let bits: Vec<u8> = self.wf_test_bits.chars()
            .filter_map(|c| match c {
                '0' => Some(0),
                '1' => Some(1),
                _ => None,
            })
            .collect();

        if bits.is_empty() {
            return;
        }

        let (bits_per_symbol, sqrt_m) = match self.selected_waveform.as_str() {
            "16-QAM" => (4, 4),
            "64-QAM" => (6, 8),
            "256-QAM" => (8, 16),
            _ => (4, 4),
        };

        let samples_per_symbol = (self.wf_sample_rate / self.wf_symbol_rate) as usize;

        // Pad bits to multiple of bits_per_symbol
        let mut padded_bits = bits.clone();
        while padded_bits.len() % bits_per_symbol != 0 {
            padded_bits.push(0);
        }

        let mut samples = Vec::new();

        // Normalization factor for unit average power
        let norm = (2.0 * (sqrt_m * sqrt_m - 1) as f64 / 3.0).sqrt();

        for chunk in padded_bits.chunks(bits_per_symbol) {
            // Split bits for I and Q
            let half = bits_per_symbol / 2;
            let mut i_idx = 0u8;
            let mut q_idx = 0u8;

            for (i, bit) in chunk[..half].iter().enumerate() {
                i_idx |= bit << (half - 1 - i);
            }
            for (i, bit) in chunk[half..].iter().enumerate() {
                q_idx |= bit << (half - 1 - i);
            }

            // Gray decode
            let i_gray = i_idx ^ (i_idx >> 1);
            let q_gray = q_idx ^ (q_idx >> 1);

            // Map to constellation: -sqrt_m+1, -sqrt_m+3, ..., sqrt_m-3, sqrt_m-1
            let i_val = (2.0 * i_gray as f64 - (sqrt_m - 1) as f64) / norm;
            let q_val = (2.0 * q_gray as f64 - (sqrt_m - 1) as f64) / norm;

            // Generate samples for this symbol
            for i in 0..samples_per_symbol {
                let t = i as f64 / self.wf_sample_rate;
                let phase = 2.0 * PI * self.wf_carrier_freq * t;
                let sample = IQSample::new(
                    (i_val * phase.cos() - q_val * phase.sin()) * self.wf_amplitude as f64,
                    (i_val * phase.sin() + q_val * phase.cos()) * self.wf_amplitude as f64,
                );
                samples.push(sample);
            }
        }

        // Apply channel effects
        if self.wf_channel_model == ChannelModel::Awgn {
            self.apply_awgn_noise(&mut samples);
        }

        // Demodulate and calculate BER
        let demod_bits = self.demodulate_qam(&samples, samples_per_symbol, bits_per_symbol);
        let compare_len = bits.len().min(demod_bits.len());
        let errors: usize = bits.iter().take(compare_len).zip(demod_bits.iter())
            .map(|(tx, rx)| if tx != rx { 1 } else { 0 })
            .sum();
        self.wf_ber = Some(errors as f64 / compare_len as f64);
        self.wf_demod_bits = Some(demod_bits);

        self.generated_samples = Some(samples);
    }

    /// Demodulate QAM signal
    fn demodulate_qam(&self, samples: &[IQSample], samples_per_symbol: usize, bits_per_symbol: usize) -> Vec<u8> {
        let sqrt_m = match self.selected_waveform.as_str() {
            "16-QAM" => 4,
            "64-QAM" => 8,
            "256-QAM" => 16,
            _ => 4,
        };

        let norm = (2.0 * (sqrt_m * sqrt_m - 1) as f64 / 3.0).sqrt();
        let half = bits_per_symbol / 2;
        let mut bits = Vec::new();

        for chunk in samples.chunks(samples_per_symbol) {
            // Average I and Q
            let avg_i: f64 = chunk.iter().map(|s| s.re).sum::<f64>() / chunk.len() as f64;
            let avg_q: f64 = chunk.iter().map(|s| s.im).sum::<f64>() / chunk.len() as f64;

            // Denormalize
            let i_val = (avg_i / self.wf_amplitude as f64) * norm;
            let q_val = (avg_q / self.wf_amplitude as f64) * norm;

            // Quantize to nearest constellation point
            let i_idx = (((i_val + (sqrt_m - 1) as f64) / 2.0 + 0.5) as i32).clamp(0, sqrt_m - 1) as u8;
            let q_idx = (((q_val + (sqrt_m - 1) as f64) / 2.0 + 0.5) as i32).clamp(0, sqrt_m - 1) as u8;

            // Reverse Gray coding
            let mut i_gray = i_idx;
            let mut i_binary = i_gray;
            while i_gray > 0 {
                i_gray >>= 1;
                i_binary ^= i_gray;
            }

            let mut q_gray = q_idx;
            let mut q_binary = q_gray;
            while q_gray > 0 {
                q_gray >>= 1;
                q_binary ^= q_gray;
            }

            // Extract bits
            for i in (0..half).rev() {
                bits.push(((i_binary >> i) & 1) as u8);
            }
            for i in (0..half).rev() {
                bits.push(((q_binary >> i) & 1) as u8);
            }
        }

        bits
    }

    /// Generate a demo signal for waveforms without full parameter controls
    fn generate_default_waveform_signal(&mut self) {
        if let Some(wf) = WaveformFactory::create(&self.selected_waveform, self.wf_sample_rate) {
            // Generate a demo signal (about 50ms worth)
            let samples = wf.generate_demo(50.0);
            self.generated_samples = Some(samples);
            self.wf_demod_bits = None;
            self.wf_ber = None;
        }
    }

    /// Create a waveform instance using sidebar parameters (for pipeline view)
    fn create_waveform_for_view(&self) -> Option<Box<dyn r4w_core::waveform::Waveform>> {
        use r4w_core::waveform::{CommonParams, WaveformFactory};
        use r4w_core::waveform::ofdm::{OFDM, SubcarrierModulation};
        use r4w_core::waveform::psk::PSK;
        use r4w_core::waveform::qam::QAM;
        use r4w_core::waveform::fsk::FSK;

        let common = CommonParams {
            sample_rate: self.wf_sample_rate,
            carrier_freq: self.wf_carrier_freq,
            amplitude: self.wf_amplitude as f64,
        };

        match self.selected_waveform.as_str() {
            // PSK variants - use sidebar symbol rate
            "BPSK" => Some(Box::new(PSK::new_bpsk(common, self.wf_symbol_rate))),
            "QPSK" => Some(Box::new(PSK::new_qpsk(common, self.wf_symbol_rate))),
            "8-PSK" | "8PSK" => Some(Box::new(PSK::new_8psk(common, self.wf_symbol_rate))),

            // QAM variants - use sidebar symbol rate
            "16-QAM" | "16QAM" | "QAM16" => Some(Box::new(QAM::new_16qam(common, self.wf_symbol_rate))),
            "64-QAM" | "64QAM" | "QAM64" => Some(Box::new(QAM::new_64qam(common, self.wf_symbol_rate))),
            "256-QAM" | "256QAM" | "QAM256" => Some(Box::new(QAM::new_256qam(common, self.wf_symbol_rate))),

            // FSK - use sidebar symbol rate and deviation
            "FSK" | "2-FSK" | "2FSK" => Some(Box::new(FSK::new_bfsk(common, self.wf_symbol_rate, self.wf_fsk_deviation))),
            "4-FSK" | "4FSK" => Some(Box::new(FSK::new_4fsk(common, self.wf_symbol_rate, self.wf_fsk_deviation))),

            // OFDM - use sidebar FFT size, subcarriers, etc.
            "OFDM" => {
                let subcarrier_mod = match self.wf_ofdm_subcarrier_mod {
                    0 => SubcarrierModulation::Bpsk,
                    1 => SubcarrierModulation::Qpsk,
                    2 => SubcarrierModulation::Qam16,
                    _ => SubcarrierModulation::Qam64,
                };
                Some(Box::new(OFDM::new(
                    common,
                    self.wf_ofdm_fft_size,
                    self.wf_ofdm_data_subcarriers,
                    self.wf_ofdm_cp_ratio,
                    subcarrier_mod,
                )))
            }

            // For other waveforms, use WaveformFactory with sample rate
            _ => WaveformFactory::create(&self.selected_waveform, self.wf_sample_rate),
        }
    }

    /// Render default waveform parameters (fallback)
    fn render_default_waveform_params(&mut self, ui: &mut egui::Ui) {
        ui.heading("Waveform Info");
        ui.add_space(8.0);

        if let Some(wf) = WaveformFactory::create(&self.selected_waveform, self.wf_sample_rate) {
            let info = wf.info();
            ui.label(format!("Name: {}", info.full_name));
            ui.label(format!("Bits/symbol: {}", info.bits_per_symbol));
            ui.label(format!("Complexity: {}/5", info.complexity));

            ui.add_space(8.0);
            ui.label("Characteristics:");
            for characteristic in info.characteristics {
                ui.label(format!("  • {}", characteristic));
            }
        }

        ui.add_space(12.0);
        ui.separator();

        // Generate demo signal button
        ui.heading("Generate");
        ui.add_space(8.0);

        ui.horizontal(|ui| {
            let needs_initial = self.auto_update && self.generated_samples.is_none();
            if ui.button("Generate Demo Signal").clicked() || needs_initial {
                self.generate_default_waveform_signal();
            }
            ui.checkbox(&mut self.auto_update, "Auto-update");
        });

        ui.add_space(12.0);
        ui.separator();

        // Signal Info
        ui.heading("Signal Info");
        ui.add_space(8.0);

        if let Some(ref samples) = self.generated_samples {
            let duration_ms = samples.len() as f64 / self.wf_sample_rate * 1000.0;
            ui.label(format!("Samples: {}", samples.len()));
            ui.label(format!("Duration: {:.2} ms", duration_ms));
            ui.label(format!("Sample Rate: {:.0} Hz", self.wf_sample_rate));
        } else {
            ui.label("No signal generated yet.");
        }

        ui.add_space(8.0);
        ui.label(egui::RichText::new("Full parameter controls coming soon.").italics().weak());
    }

    /// Render parameters for LoRa mode
    fn render_lora_params(&mut self, ui: &mut egui::Ui) {
        ui.heading("LoRa Parameters");
        ui.add_space(8.0);

        let mut params_changed = false;

        // Spreading Factor
        ui.horizontal(|ui| {
            ui.label("Spreading Factor:");
            let sf_slider = egui::Slider::new(&mut self.sf_value, 7..=12).text("SF");
            if ui.add(sf_slider).changed() {
                params_changed = true;
            }
        });

        // Bandwidth
        ui.horizontal(|ui| {
            ui.label("Bandwidth:");
            egui::ComboBox::from_id_salt("bw_select")
                .selected_text(format!("{} kHz", self.bw_khz))
                .show_ui(ui, |ui| {
                    if ui.selectable_value(&mut self.bw_khz, 125, "125 kHz").changed() {
                        params_changed = true;
                    }
                    if ui.selectable_value(&mut self.bw_khz, 250, "250 kHz").changed() {
                        params_changed = true;
                    }
                    if ui.selectable_value(&mut self.bw_khz, 500, "500 kHz").changed() {
                        params_changed = true;
                    }
                });
        });

        // Coding Rate
        ui.horizontal(|ui| {
            ui.label("Coding Rate:");
            egui::ComboBox::from_id_salt("cr_select")
                .selected_text(format!("4/{}", 4 + self.cr_value))
                .show_ui(ui, |ui| {
                    for cr in 1..=4 {
                        if ui
                            .selectable_value(&mut self.cr_value, cr, format!("4/{}", 4 + cr))
                            .changed()
                        {
                            params_changed = true;
                        }
                    }
                });
        });

        ui.add_space(12.0);
        ui.separator();

        // Channel Parameters
        ui.heading("Channel Model");
        ui.add_space(8.0);

        // SNR
        ui.horizontal(|ui| {
            ui.label("SNR (dB):");
            let snr_slider = egui::Slider::new(&mut self.snr_db, -20.0..=40.0);
            if ui.add(snr_slider).changed() {
                params_changed = true;
            }
        });

        // Channel Model
        ui.horizontal(|ui| {
            ui.label("Model:");
            egui::ComboBox::from_id_salt("channel_model")
                .selected_text(format!("{:?}", self.channel_model))
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_value(&mut self.channel_model, ChannelModel::Ideal, "Ideal")
                        .changed()
                    {
                        params_changed = true;
                    }
                    if ui
                        .selectable_value(&mut self.channel_model, ChannelModel::Awgn, "AWGN")
                        .changed()
                    {
                        params_changed = true;
                    }
                    if ui
                        .selectable_value(
                            &mut self.channel_model,
                            ChannelModel::AwgnWithCfo,
                            "AWGN + CFO",
                        )
                        .changed()
                    {
                        params_changed = true;
                    }
                });
        });

        // CFO
        if self.channel_model == ChannelModel::AwgnWithCfo {
            ui.horizontal(|ui| {
                ui.label("CFO (Hz):");
                let cfo_slider = egui::Slider::new(&mut self.cfo_hz, -5000.0..=5000.0);
                if ui.add(cfo_slider).changed() {
                    params_changed = true;
                }
            });
            self.cfo_enabled = true;
        } else {
            self.cfo_enabled = false;
        }

        ui.add_space(12.0);
        ui.separator();

        // Payload
        ui.heading("Test Payload");
        ui.add_space(8.0);

        ui.horizontal(|ui| {
            ui.label("Message:");
        });
        if ui.text_edit_singleline(&mut self.payload).changed() {
            params_changed = true;
        }

        ui.add_space(12.0);

        // Generate button
        ui.horizontal(|ui| {
            let needs_initial = self.auto_update && self.generated_samples.is_none();
            if ui.button("Generate Signal").clicked() || needs_initial {
                self.generate_signal();
            }
            ui.checkbox(&mut self.auto_update, "Auto-update");
        });

        // Apply parameter changes
        if params_changed {
            self.update_params();
        }

        ui.add_space(20.0);
        ui.separator();

        // Statistics
        ui.heading("Signal Info");
        ui.add_space(8.0);

        if let Some(ref samples) = self.generated_samples {
            ui.label(format!("Samples: {}", samples.len()));
            ui.label(format!(
                "Duration: {:.2} ms",
                samples.len() as f64 / self.params.sample_rate * 1000.0
            ));
            ui.label(format!("Bit rate: {:.0} bps", self.params.bit_rate()));
            ui.label(format!(
                "Sensitivity: {:.1} dBm",
                self.params.sensitivity()
            ));
        }
    }

    /// Render the main content area
    fn render_content(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            // Header
            ui.horizontal(|ui| {
                ui.heading(self.active_view.name());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(self.active_view.description());
                });
            });
            ui.separator();

            // Content
            match self.active_view {
                ActiveView::Overview => {
                    self.overview_view.render(ui, &self.selected_waveform, &self.params, &self.generated_samples);
                }
                ActiveView::Waveforms => {
                    // Pass sidebar parameters to the waveform view
                    let params = WaveformParams {
                        waveform_name: self.selected_waveform.clone(),
                        sample_rate: self.wf_sample_rate,
                        test_bits: self.wf_test_bits.clone(),
                        snr_db: self.wf_snr_db,
                        channel_model: self.wf_channel_model,
                        samples: self.generated_samples.clone(),
                        ber: self.wf_ber,
                    };
                    self.waveform_view.render_with_params(ui, Some(&params));
                }
                ActiveView::WaveformWizard => {
                    self.waveform_wizard_view.render(ui);
                }
                ActiveView::CodeExplorer => {
                    self.code_explorer_view.render_with_waveform(ui, Some(&self.selected_waveform));
                }
                ActiveView::AdsbDecoder => {
                    self.adsb_view.render(ui);
                }
                ActiveView::Chirp => {
                    self.chirp_view.render(ui, &self.params, &self.generated_samples);
                }
                ActiveView::Modulation => {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        if is_lora_waveform(&self.selected_waveform) {
                            self.modulation_view.render(
                                ui,
                                &self.params,
                                &self.generated_samples,
                                self.modulator.as_ref(),
                            );
                        } else {
                            // Use generic modulation view for non-LoRa waveforms
                            if let Some(wf) = WaveformFactory::create(&self.selected_waveform, self.wf_sample_rate) {
                                let test_data = self.get_test_data();
                                self.generic_mod_view.render(
                                    ui,
                                    wf.as_ref(),
                                    &test_data,
                                    &self.generated_samples,
                                );
                            } else {
                                ui.label("Waveform not available for generic view.");
                            }
                        }
                    });
                }
                ActiveView::Demodulation => {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        if is_lora_waveform(&self.selected_waveform) {
                            self.demodulation_view.render(
                                ui,
                                &self.params,
                                &self.generated_samples,
                                &mut self.channel,
                                self.demodulator.as_mut(),
                            );
                        } else {
                            // Use generic demodulation view for non-LoRa waveforms
                            if let Some(wf) = WaveformFactory::create(&self.selected_waveform, self.wf_sample_rate) {
                                self.generic_demod_view.render(
                                    ui,
                                    wf.as_ref(),
                                    &self.generated_samples,
                                );
                            } else {
                                ui.label("Waveform not available for generic view.");
                            }
                        }
                    });
                }
                ActiveView::Pipeline => {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        if is_lora_waveform(&self.selected_waveform) {
                            self.pipeline_view.render(
                                ui,
                                &self.params,
                                &self.payload,
                                &mut self.modulator,
                                &mut self.channel,
                                &mut self.demodulator,
                            );
                        } else {
                            // Use generic pipeline view for non-LoRa waveforms
                            // Use create_waveform_for_view() to get waveform with sidebar parameters
                            if let Some(wf) = self.create_waveform_for_view() {
                                let test_data = self.get_test_data();
                                self.generic_pipeline_view.render(
                                    ui,
                                    wf.as_ref(),
                                    &test_data,
                                    &self.generated_samples,
                                );
                            } else {
                                ui.label("Waveform not available for generic view.");
                            }
                        }
                    });
                }
                ActiveView::Spectrum => {
                    // Use appropriate sample rate based on waveform type
                    let sample_rate = if is_lora_waveform(&self.selected_waveform) {
                        self.params.sample_rate
                    } else {
                        self.wf_sample_rate
                    };
                    self.spectrum_view.render(ui, sample_rate, &self.selected_waveform, &self.generated_samples);
                }
                ActiveView::Constellation => {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        self.constellation_view.render(ui, &self.selected_waveform, &self.generated_samples);
                    });
                }
                ActiveView::Streaming => {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        self.streaming_view.render(ui, &mut self.stream_manager);
                    });
                }
                ActiveView::UdpBenchmark => {
                    self.udp_benchmark_view.render(ui);
                }
                ActiveView::RemoteLab => {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        self.remote_lab_view.render(ui);
                    });
                }
                ActiveView::FhssLab => {
                    self.fhss_view.render(ui);
                }
                ActiveView::Stanag4285Lab => {
                    self.stanag_view.render(ui);
                }
                ActiveView::AleLab => {
                    self.ale_view.render(ui);
                }
                ActiveView::MeshNetwork => {
                    self.mesh_network_view.render(ui);
                }
                ActiveView::Performance => {
                    self.performance_view.render(ui);
                }
                ActiveView::WaveformComparison => {
                    self.waveform_comparison_view.render(ui);
                }
            }
        });
    }
}

impl eframe::App for WaveformExplorer {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Tick the stream manager for real-time playback
        if self.stream_manager.tick() {
            // Stream updated, request repaint for animation
            ctx.request_repaint();
        }

        // Keep repainting while playing
        if self.stream_manager.state == PlaybackState::Playing {
            ctx.request_repaint();
        }

        self.render_side_panel(ctx);
        self.render_content(ctx);
    }
}
