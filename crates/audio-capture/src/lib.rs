use anyhow::{bail, Result};

#[derive(Debug, Clone, Copy)]
pub struct AudioFormat {
    pub sample_rate: u32,
    pub channels: u16,
    pub bits_per_sample: u16,
}

pub trait LoopbackCapture {
    fn format(&self) -> AudioFormat;
    fn start(&mut self) -> Result<()>;
    fn stop(&mut self) -> Result<()>;
}

#[cfg(windows)]
pub struct WasapiLoopback {
    format: AudioFormat,
}

#[cfg(windows)]
impl WasapiLoopback {
    pub fn new() -> Result<Self> {
        Ok(Self {
            format: AudioFormat {
                sample_rate: 48_000,
                channels: 2,
                bits_per_sample: 32,
            },
        })
    }
}

#[cfg(windows)]
impl LoopbackCapture for WasapiLoopback {
    fn format(&self) -> AudioFormat {
        self.format
    }

    fn start(&mut self) -> Result<()> {
        // v0.1.0 bootstrap: interface is fixed now; event-driven WASAPI
        // loopback implementation is the next milestone.
        bail!("WASAPI loopback engine not wired yet")
    }

    fn stop(&mut self) -> Result<()> {
        Ok(())
    }
}
