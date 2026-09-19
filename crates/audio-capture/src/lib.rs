use anyhow::{anyhow, Result};
use crossbeam_channel::{bounded, Receiver, RecvTimeoutError};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

#[derive(Debug, Clone, Copy)]
pub struct AudioFormat {
    pub sample_rate: u32,
    pub channels: u16,
    pub bits_per_sample: u16,
}

impl Default for AudioFormat {
    fn default() -> Self {
        Self {
            sample_rate: 44_100,
            channels: 2,
            bits_per_sample: 16,
        }
    }
}

#[derive(Debug)]
pub struct CapturedChunk {
    pub samples: Vec<i16>,
    pub sample_rate: u32,
    pub channels: u16,
}

pub struct CaptureHandle {
    pub device_name: String,
    pub format: AudioFormat,
    rx: Receiver<CapturedChunk>,
    running: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl CaptureHandle {
    pub fn recv_timeout(&self, timeout: Duration) -> Result<CapturedChunk> {
        match self.poll_timeout(timeout)? {
            Some(chunk) => Ok(chunk),
            None => Err(anyhow!("WASAPI capture timeout")),
        }
    }

    /// Poll for PCM without treating a quiet Windows endpoint as a failure.
    ///
    /// Ok(None) means there was no audio packet inside the requested window.
    /// A disconnected channel still reports a real capture-engine failure.
    pub fn poll_timeout(&self, timeout: Duration) -> Result<Option<CapturedChunk>> {
        match self.rx.recv_timeout(timeout) {
            Ok(chunk) => Ok(Some(chunk)),
            Err(RecvTimeoutError::Timeout) => Ok(None),
            Err(RecvTimeoutError::Disconnected) => {
                Err(anyhow!("WASAPI capture worker disconnected"))
            }
        }
    }

    pub fn stop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for CaptureHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(windows)]
pub fn start_default_loopback(format: AudioFormat) -> Result<CaptureHandle> {
    use wasapi::{
        get_default_device, initialize_mta, Direction, SampleType, ShareMode, WaveFormat,
    };

    if format.channels != 2 || format.bits_per_sample != 16 {
        return Err(anyhow!("SolYan AirPlay2 capture requires stereo 16-bit PCM"));
    }

    const CHUNK_FRAMES: usize = 352;
    const EVENT_POLL_MS: u32 = 250;

    let (tx, rx) = bounded::<CapturedChunk>(64);
    let (init_tx, init_rx) = std::sync::mpsc::sync_channel::<Result<String, String>>(1);
    let running = Arc::new(AtomicBool::new(true));
    let thread_running = Arc::clone(&running);

    let target_rate = format.sample_rate;
    let target_channels = format.channels;

    let handle = thread::Builder::new()
        .name("solyan-wasapi-loopback".into())
        .spawn(move || {
            let result = (|| -> Result<(), String> {
                initialize_mta()
                    .ok()
                    .map_err(|e| format!("initialize_mta: {e}"))?;

                let device = get_default_device(&Direction::Render)
                    .map_err(|e| format!("get_default_device: {e}"))?;
                let device_name = device
                    .get_friendlyname()
                    .unwrap_or_else(|_| "Default Windows render device".to_string());

                let mut client = device
                    .get_iaudioclient()
                    .map_err(|e| format!("get_iaudioclient: {e}"))?;

                let wave_format = WaveFormat::new(
                    16,
                    16,
                    &SampleType::Int,
                    target_rate as usize,
                    target_channels as usize,
                    None,
                );
                let bytes_per_frame = wave_format.get_blockalign() as usize;
                let (default_period, _) = client
                    .get_periods()
                    .map_err(|e| format!("get_periods: {e}"))?;

                client
                    .initialize_client(
                        &wave_format,
                        default_period,
                        &Direction::Capture,
                        &ShareMode::Shared,
                        true,
                    )
                    .map_err(|e| format!("initialize loopback client: {e}"))?;

                let event = client
                    .set_get_eventhandle()
                    .map_err(|e| format!("set_get_eventhandle: {e}"))?;
                let capture = client
                    .get_audiocaptureclient()
                    .map_err(|e| format!("get_audiocaptureclient: {e}"))?;

                client
                    .start_stream()
                    .map_err(|e| format!("start_stream: {e}"))?;

                let _ = init_tx.send(Ok(device_name));
                let mut bytes = VecDeque::<u8>::with_capacity(CHUNK_FRAMES * bytes_per_frame * 8);
                let chunk_bytes = CHUNK_FRAMES * bytes_per_frame;

                while thread_running.load(Ordering::SeqCst) {
                    if event.wait_for_event(EVENT_POLL_MS).is_err() {
                        continue;
                    }

                    capture
                        .read_from_device_to_deque(&mut bytes)
                        .map_err(|e| format!("capture read: {e}"))?;

                    while bytes.len() >= chunk_bytes {
                        let mut samples =
                            Vec::with_capacity(CHUNK_FRAMES * target_channels as usize);
                        for _ in 0..(CHUNK_FRAMES * target_channels as usize) {
                            let lo = bytes.pop_front().unwrap();
                            let hi = bytes.pop_front().unwrap();
                            samples.push(i16::from_le_bytes([lo, hi]));
                        }

                        let _ = tx.try_send(CapturedChunk {
                            samples,
                            sample_rate: target_rate,
                            channels: target_channels,
                        });
                    }
                }

                let _ = client.stop_stream();
                Ok(())
            })();

            if let Err(error) = result {
                let _ = init_tx.send(Err(error));
            }
        })?;

    let device_name = match init_rx.recv_timeout(Duration::from_secs(5)) {
        Ok(Ok(name)) => name,
        Ok(Err(error)) => return Err(anyhow!(error)),
        Err(error) => return Err(anyhow!("WASAPI init timeout: {error}")),
    };

    Ok(CaptureHandle {
        device_name,
        format,
        rx,
        running,
        thread: Some(handle),
    })
}

#[cfg(not(windows))]
pub fn start_default_loopback(_format: AudioFormat) -> Result<CaptureHandle> {
    Err(anyhow!("WASAPI loopback is available only on Windows"))
}
