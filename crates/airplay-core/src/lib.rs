pub mod discovery;
pub mod session;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamMode {
    RealtimeAlac,
    Buffered,
}

#[derive(Debug, Clone)]
pub struct ReceiverCapabilities {
    pub supports_airplay2: bool,
    pub supports_ptp: bool,
    pub supports_buffered_audio: bool,
    pub latency_min_samples: Option<u32>,
    pub latency_max_samples: Option<u32>,
}

impl Default for ReceiverCapabilities {
    fn default() -> Self {
        Self {
            supports_airplay2: false,
            supports_ptp: false,
            supports_buffered_audio: false,
            latency_min_samples: None,
            latency_max_samples: None,
        }
    }
}
