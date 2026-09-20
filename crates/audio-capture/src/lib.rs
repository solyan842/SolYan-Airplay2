use anyhow::{anyhow, Result};
use crossbeam_channel::{unbounded, Receiver, RecvTimeoutError};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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
    runtime_error_rx: Receiver<String>,
    running: Arc<AtomicBool>,
    captured_frames: Arc<AtomicU64>,
    thread: Option<thread::JoinHandle<()>>,
}

impl CaptureHandle {
    pub fn recv_timeout(&self, timeout: Duration) -> Result<CapturedChunk> {
        match self.poll_timeout(timeout)? {
            Some(chunk) => Ok(chunk),
            None => Err(anyhow!("WASAPI capture timeout")),
        }
    }

    pub fn poll_timeout(&self, timeout: Duration) -> Result<Option<CapturedChunk>> {
        match self.rx.recv_timeout(timeout) {
            Ok(chunk) => Ok(Some(chunk)),
            Err(RecvTimeoutError::Timeout) => Ok(None),
            Err(RecvTimeoutError::Disconnected) => {
                let detail = self
                    .runtime_error_rx
                    .try_recv()
                    .unwrap_or_else(|_| "worker exited without a detailed WASAPI error".to_string());
                Err(anyhow!("WASAPI capture worker disconnected: {detail}"))
            }
        }
    }

    pub fn total_frames(&self) -> u64 {
        self.captured_frames.load(Ordering::Relaxed)
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
#[derive(Debug, Clone, Copy)]
enum NativeSampleKind {
    S16,
    F32,
}

#[cfg(windows)]
fn set_capture_thread_priority() {
    use windows_sys::Win32::System::Threading::{
        AvSetMmThreadCharacteristicsW, AvSetMmThreadPriority, GetCurrentThread,
        SetThreadPriority, AVRT_PRIORITY_HIGH, THREAD_PRIORITY_HIGHEST,
    };

    let task_name: Vec<u16> = "Pro Audio\0".encode_utf16().collect();
    let mut task_index: u32 = 0;

    unsafe {
        let handle = AvSetMmThreadCharacteristicsW(task_name.as_ptr(), &mut task_index);
        if !handle.is_null() {
            let _ = AvSetMmThreadPriority(handle, AVRT_PRIORITY_HIGH);
            tracing::info!(
                "WASAPI capture registered with Windows MMCSS 'Pro Audio' (task index {})",
                task_index
            );
            return;
        }

        let ok = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_HIGHEST);
        if ok != 0 {
            tracing::info!("WASAPI capture priority set to THREAD_PRIORITY_HIGHEST");
        } else {
            tracing::warn!(
                "Failed to elevate WASAPI capture thread priority: {}",
                std::io::Error::last_os_error()
            );
        }
    }
}

#[cfg(windows)]
fn convert_native_to_stereo_i16(
    raw: &[u8],
    frames: usize,
    channels: usize,
    kind: NativeSampleKind,
) -> Vec<i16> {
    let mut out = Vec::with_capacity(frames * 2);

    match kind {
        NativeSampleKind::S16 => {
            let bytes_per_sample = 2usize;
            for frame in 0..frames {
                let sample_at = |channel: usize| -> i16 {
                    let ch = channel.min(channels.saturating_sub(1));
                    let idx = (frame * channels + ch) * bytes_per_sample;
                    if idx + 1 >= raw.len() {
                        return 0;
                    }
                    i16::from_le_bytes([raw[idx], raw[idx + 1]])
                };

                let left = sample_at(0);
                let right = if channels > 1 { sample_at(1) } else { left };
                out.push(left);
                out.push(right);
            }
        }
        NativeSampleKind::F32 => {
            let bytes_per_sample = 4usize;
            for frame in 0..frames {
                let sample_at = |channel: usize| -> i16 {
                    let ch = channel.min(channels.saturating_sub(1));
                    let idx = (frame * channels + ch) * bytes_per_sample;
                    if idx + 3 >= raw.len() {
                        return 0;
                    }
                    let f = f32::from_le_bytes([
                        raw[idx],
                        raw[idx + 1],
                        raw[idx + 2],
                        raw[idx + 3],
                    ])
                    .clamp(-1.0, 1.0);
                    (f * 32767.0).round() as i16
                };

                let left = sample_at(0);
                let right = if channels > 1 { sample_at(1) } else { left };
                out.push(left);
                out.push(right);
            }
        }
    }

    out
}

#[cfg(windows)]
pub fn start_default_loopback(_requested: AudioFormat) -> Result<CaptureHandle> {
    use wasapi::{DeviceEnumerator, Direction, SampleType, StreamMode, initialize_mta};

    // Event mode normally wakes immediately. If Windows misses or delays a
    // loopback event, inspect the capture buffer again after 20ms instead of
    // starving the AirPlay pipeline for a quarter of a second.
    const EVENT_POLL_MS: u32 = 20;
    const OUTPUT_BLOCK_FRAMES: usize = 1024;

    // Capture must be lossless. The previous bounded queue + try_send could
    // silently drop a PCM block during a short scheduler stall, producing clicks.
    let (tx, rx) = unbounded::<CapturedChunk>();
    let (runtime_error_tx, runtime_error_rx) = unbounded::<String>();
    let (init_tx, init_rx) =
        std::sync::mpsc::sync_channel::<Result<(String, AudioFormat), String>>(1);
    let running = Arc::new(AtomicBool::new(true));
    let thread_running = Arc::clone(&running);
    let captured_frames = Arc::new(AtomicU64::new(0));
    let thread_captured_frames = Arc::clone(&captured_frames);

    let handle = thread::Builder::new()
        .name("solyan-wasapi-loopback".into())
        .spawn(move || {
            let result = (|| -> Result<(), String> {
                initialize_mta()
                    .ok()
                    .map_err(|e| format!("initialize_mta: {e}"))?;
                set_capture_thread_priority();

                let enumerator = DeviceEnumerator::new()
                    .map_err(|e| format!("DeviceEnumerator::new: {e}"))?;
                let device = enumerator
                    .get_default_device(&Direction::Render)
                    .map_err(|e| format!("get_default_device: {e}"))?;
                let device_name = device
                    .get_friendlyname()
                    .unwrap_or_else(|_| "Default Windows render device".to_string());

                let mut client = device
                    .get_iaudioclient()
                    .map_err(|e| format!("get_iaudioclient: {e}"))?;

                // Capture in the endpoint's native shared-mode format. Avoid asking
                // Windows Audio Engine to convert 48k float -> 44.1k int in real time.
                let mix_format = client
                    .get_mixformat()
                    .map_err(|e| format!("get_mixformat: {e}"))?;
                let native_rate = mix_format.get_samplespersec();
                let native_channels = mix_format.get_nchannels() as usize;
                let native_bits = mix_format.get_bitspersample();
                let native_kind = match mix_format
                    .get_subformat()
                    .map_err(|e| format!("get_subformat: {e}"))?
                {
                    SampleType::Float if native_bits == 32 => NativeSampleKind::F32,
                    SampleType::Int if native_bits == 16 => NativeSampleKind::S16,
                    other => {
                        return Err(format!(
                            "unsupported WASAPI native format: {:?}, {} bit, {} ch @ {} Hz",
                            other, native_bits, native_channels, native_rate
                        ))
                    }
                };

                if native_channels == 0 {
                    return Err("WASAPI native format has zero channels".into());
                }

                let (_default_period, min_period) = client
                    .get_device_period()
                    .map_err(|e| format!("get_device_period: {e}"))?;

                let mode = StreamMode::EventsShared {
                    autoconvert: false,
                    buffer_duration_hns: min_period,
                };

                client
                    .initialize_client(&mix_format, &Direction::Capture, &mode)
                    .map_err(|e| format!("initialize native loopback client: {e}"))?;

                let event = client
                    .set_get_eventhandle()
                    .map_err(|e| format!("set_get_eventhandle: {e}"))?;
                let capture = client
                    .get_audiocaptureclient()
                    .map_err(|e| format!("get_audiocaptureclient: {e}"))?;

                client
                    .start_stream()
                    .map_err(|e| format!("start_stream: {e}"))?;

                tracing::info!(
                    "Native WASAPI loopback: {} Hz, {} ch, {} bit {:?} -> stereo i16",
                    native_rate,
                    native_channels,
                    native_bits,
                    native_kind
                );

                let output_format = AudioFormat {
                    sample_rate: native_rate,
                    channels: 2,
                    bits_per_sample: 16,
                };
                let _ = init_tx.send(Ok((device_name, output_format)));

                let mut raw = VecDeque::<u8>::new();
                let mut pcm = VecDeque::<i16>::with_capacity(OUTPUT_BLOCK_FRAMES * 2 * 4);
                let output_samples_per_block = OUTPUT_BLOCK_FRAMES * 2;

                while thread_running.load(Ordering::SeqCst) {
                    // Do not make PCM capture depend exclusively on the event signal.
                    // On a cold first launch Windows can occasionally miss/delay the
                    // initial loopback event even though packets are already pending.
                    // We still wait to avoid busy-spinning, but always inspect the
                    // capture buffer afterwards and drain anything available.
                    let _ = event.wait_for_event(EVENT_POLL_MS);

                    // Drain every packet currently pending. This also recovers from
                    // a missed/late first event without requiring an app restart.
                    loop {
                        let pending = capture
                            .get_next_packet_size()
                            .map_err(|e| format!("get_next_packet_size: {e}"))?;
                        let Some(frames) = pending else {
                            break;
                        };
                        if frames == 0 {
                            break;
                        }

                        raw.clear();
                        capture
                            .read_from_device_to_deque(&mut raw)
                            .map_err(|e| format!("capture read: {e}"))?;

                        thread_captured_frames.fetch_add(frames as u64, Ordering::Relaxed);
                        let bytes: Vec<u8> = raw.drain(..).collect();
                        let converted = convert_native_to_stereo_i16(
                            &bytes,
                            frames as usize,
                            native_channels,
                            native_kind,
                        );
                        pcm.extend(converted);

                        while pcm.len() >= output_samples_per_block {
                            let mut samples = Vec::with_capacity(output_samples_per_block);
                            for _ in 0..output_samples_per_block {
                                samples.push(pcm.pop_front().unwrap());
                            }

                            // Unbounded handoff intentionally preserves continuity.
                            // Consumer backpressure is handled later in the AirPlay
                            // decoder queue; capture itself never drops a block.
                            if tx
                                .send(CapturedChunk {
                                    samples,
                                    sample_rate: native_rate,
                                    channels: 2,
                                })
                                .is_err()
                            {
                                let _ = client.stop_stream();
                                return Ok(());
                            }
                        }
                    }
                }

                let _ = client.stop_stream();
                Ok(())
            })();

            if let Err(error) = result {
                // If initialization already succeeded, init_rx is gone; retain
                // the actual runtime WASAPI failure for the live supervisor.
                let _ = runtime_error_tx.send(error.clone());
                let _ = init_tx.send(Err(error));
            }
        })?;

    let (device_name, format) = match init_rx.recv_timeout(Duration::from_secs(5)) {
        Ok(Ok(info)) => info,
        Ok(Err(error)) => return Err(anyhow!(error)),
        Err(error) => return Err(anyhow!("WASAPI init timeout: {error}")),
    };

    Ok(CaptureHandle {
        device_name,
        format,
        rx,
        runtime_error_rx,
        running,
        captured_frames,
        thread: Some(handle),
    })
}

#[cfg(not(windows))]
pub fn start_default_loopback(_format: AudioFormat) -> Result<CaptureHandle> {
    Err(anyhow!("WASAPI loopback is available only on Windows"))
}
