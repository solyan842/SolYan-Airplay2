use airplay2_audio::{LiveAudioDecoder, LivePcmFrame};
use airplay2_client::AirPlayClient;
use airplay2_core::Device;
use anyhow::{anyhow, bail, Result};
use audio_capture::{start_default_loopback, AudioFormat as CaptureFormat};
use crossbeam_channel::Sender;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveStreamMode {
    Single,
    MultiroomExperimental,
}

#[derive(Clone)]
pub struct StreamControl {
    stop: Arc<AtomicBool>,
    volume_bits: Arc<AtomicU32>,
}

impl StreamControl {
    pub fn new(initial_volume: f32) -> Self {
        Self {
            stop: Arc::new(AtomicBool::new(false)),
            volume_bits: Arc::new(AtomicU32::new(initial_volume.clamp(0.0, 1.0).to_bits())),
        }
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::Release);
    }

    pub fn is_stopped(&self) -> bool {
        self.stop.load(Ordering::Acquire)
    }

    pub fn set_volume(&self, volume: f32) {
        self.volume_bits
            .store(volume.clamp(0.0, 1.0).to_bits(), Ordering::Release);
    }

    pub fn volume(&self) -> f32 {
        f32::from_bits(self.volume_bits.load(Ordering::Acquire)).clamp(0.0, 1.0)
    }
}

#[derive(Debug, Clone, Default)]
pub struct StreamProgress {
    pub elapsed_secs: f64,
    pub captured_chunks: u64,
    pub silence_chunks: u64,
    pub dropped_chunks: u64,
    pub packets_sent: u64,
    pub retransmit_requested: u64,
    pub retransmit_fulfilled: u64,
    pub underruns: u64,
    pub loss_percent: f64,
    pub target_count: usize,
}

#[derive(Debug, Clone)]
pub struct LiveStreamResult {
    pub target_names: Vec<String>,
    pub captured_chunks: u64,
    pub silence_chunks: u64,
    pub dropped_chunks: u64,
    pub elapsed: Duration,
}

async fn discover_targets(
    client: &AirPlayClient,
    selectors: &[String],
    mode: LiveStreamMode,
    discovery_timeout: Duration,
) -> Result<Vec<Device>> {
    let mut devices = client.discover(discovery_timeout).await?;
    devices.retain(|d| d.features.supports_audio());

    if devices.is_empty() {
        bail!("no AirPlay audio receivers found");
    }

    if selectors.is_empty() {
        return match mode {
            LiveStreamMode::Single if devices.len() == 1 => Ok(vec![devices.remove(0)]),
            LiveStreamMode::Single => {
                let homepods: Vec<_> = devices
                    .iter()
                    .filter(|d| {
                        let model = d.model.to_lowercase();
                        let name = d.name.to_lowercase();
                        model.contains("audioaccessory") || name.contains("homepod")
                    })
                    .cloned()
                    .collect();

                if homepods.len() == 1 {
                    Ok(homepods)
                } else {
                    bail!("select one AirPlay receiver before starting the stream")
                }
            }
            LiveStreamMode::MultiroomExperimental => {
                bail!("select at least two AirPlay receivers for multiroom")
            }
        };
    }

    let mut out = Vec::new();
    let mut seen = HashSet::new();

    for selector in selectors {
        let needle = selector.to_lowercase();
        let device = devices
            .iter()
            .find(|d| {
                d.id.to_mac_string().to_lowercase() == needle
                    || d.name.to_lowercase() == needle
                    || d.name.to_lowercase().contains(&needle)
                    || d.addresses.iter().any(|ip| ip.to_string() == selector.as_str())
            })
            .cloned()
            .ok_or_else(|| anyhow!("AirPlay receiver not found: {selector}"))?;

        let key = device.id.to_mac_string();
        if seen.insert(key) {
            out.push(device);
        }
    }

    match mode {
        LiveStreamMode::Single if out.len() != 1 => {
            bail!("single-speaker mode requires exactly one receiver")
        }
        LiveStreamMode::MultiroomExperimental if out.len() < 2 => {
            bail!("multiroom requires at least two receivers")
        }
        _ => Ok(out),
    }
}

pub async fn run_live_stream(
    selectors: Vec<String>,
    mode: LiveStreamMode,
    control: StreamControl,
    progress_tx: Option<Sender<StreamProgress>>,
    render_delay_ms: u32,
) -> Result<LiveStreamResult> {
    const SAMPLE_RATE: u32 = 44_100;
    const CHANNELS: u8 = 2;
    const CHUNK_FRAMES: usize = 352;
    const PREBUFFER_CHUNKS: u64 = 64;
    const LIVE_QUEUE_CHUNKS: usize = 96;
    const FRAME_WAIT: Duration = Duration::from_millis(8);

    let mut client = AirPlayClient::new()?;
    client.set_render_delay_ms(render_delay_ms);

    let targets =
        discover_targets(&client, &selectors, mode, Duration::from_secs(4)).await?;
    let target_names = targets.iter().map(|d| d.name.clone()).collect::<Vec<_>>();

    match mode {
        LiveStreamMode::Single => {
            tokio::time::timeout(Duration::from_secs(20), client.connect(&targets[0]))
                .await
                .map_err(|_| anyhow!("AirPlay connection timed out"))??;
        }
        LiveStreamMode::MultiroomExperimental => {
            tokio::time::timeout(Duration::from_secs(45), client.connect_group(&targets))
                .await
                .map_err(|_| anyhow!("AirPlay multiroom group connection timed out"))??;
        }
    }

    let mut capture = start_default_loopback(CaptureFormat::default())?;
    if capture.format.sample_rate != SAMPLE_RATE
        || capture.format.channels != CHANNELS as u16
        || capture.format.bits_per_sample != 16
    {
        let _ = client.disconnect().await;
        bail!(
            "unexpected WASAPI format: {} Hz / {} ch / {} bit",
            capture.format.sample_rate,
            capture.format.channels,
            capture.format.bits_per_sample
        );
    }

    let (sender, decoder) =
        LiveAudioDecoder::create_pair(SAMPLE_RATE, CHANNELS, LIVE_QUEUE_CHUNKS);

    // AirPlay needs data ready before RECORD starts. A quiet Windows endpoint is
    // perfectly valid, so seed the decoder with real PCM when available and
    // synthetic silence otherwise. Audio can begin later without reconnecting.
    let mut prebuffered = 0u64;
    let mut silence_chunks = 0u64;
    while prebuffered < PREBUFFER_CHUNKS && !control.is_stopped() {
        let frame = match capture.poll_timeout(FRAME_WAIT)? {
            Some(chunk) => LivePcmFrame {
                samples: chunk.samples,
                channels: chunk.channels as u8,
                sample_rate: chunk.sample_rate,
            },
            None => {
                silence_chunks += 1;
                LivePcmFrame {
                    samples: vec![0; CHUNK_FRAMES * CHANNELS as usize],
                    channels: CHANNELS,
                    sample_rate: SAMPLE_RATE,
                }
            }
        };

        if sender.try_send(frame) {
            prebuffered += 1;
        }
    }

    if control.is_stopped() {
        capture.stop();
        let _ = client.disconnect().await;
        bail!("stream cancelled before playback started");
    }

    match mode {
        LiveStreamMode::Single => client.start_live_streaming_with_decoder(decoder).await?,
        LiveStreamMode::MultiroomExperimental => {
            client.start_live_streaming_to_group(decoder).await?
        }
    }

    let mut applied_volume = control.volume();
    client.set_volume(applied_volume).await?;

    let started = Instant::now();
    let mut captured_chunks = 0u64;
    let mut dropped_chunks = 0u64;
    let mut next_feedback = Instant::now() + Duration::from_secs(2);
    let mut next_progress = Instant::now() + Duration::from_millis(500);

    while !control.is_stopped() {
        let (frame, is_silence) = match capture.poll_timeout(FRAME_WAIT)? {
            Some(chunk) => (
                LivePcmFrame {
                    samples: chunk.samples,
                    channels: chunk.channels as u8,
                    sample_rate: chunk.sample_rate,
                },
                false,
            ),
            None => (
                LivePcmFrame {
                    samples: vec![0; CHUNK_FRAMES * CHANNELS as usize],
                    channels: CHANNELS,
                    sample_rate: SAMPLE_RATE,
                },
                true,
            ),
        };

        // Preserve PCM continuity. Blocking briefly here is preferable to dropping
        // a 352-frame chunk, which creates an audible discontinuity/click.
        if sender.send(frame) {
            if is_silence {
                silence_chunks += 1;
            } else {
                captured_chunks += 1;
            }
        } else {
            dropped_chunks += 1;
        }

        let desired_volume = control.volume();
        if (desired_volume - applied_volume).abs() > 0.005 {
            if client.set_volume(desired_volume).await.is_ok() {
                applied_volume = desired_volume;
            }
        }

        let now = Instant::now();
        if now >= next_feedback {
            let _ = client.send_feedback().await;
            next_feedback = now + Duration::from_secs(2);
        }

        if now >= next_progress {
            let stats = client.stats_snapshot();
            if let Some(tx) = &progress_tx {
                let _ = tx.try_send(StreamProgress {
                    elapsed_secs: started.elapsed().as_secs_f64(),
                    captured_chunks,
                    silence_chunks,
                    dropped_chunks,
                    packets_sent: stats.packets_sent,
                    retransmit_requested: stats.rtx_requested,
                    retransmit_fulfilled: stats.rtx_fulfilled,
                    underruns: stats.underruns,
                    loss_percent: stats.loss_percent(),
                    target_count: targets.len(),
                });
            }
            next_progress = now + Duration::from_millis(500);
        }
    }

    capture.stop();
    drop(sender);
    let _ = client.stop().await;
    let _ = client.disconnect().await;

    Ok(LiveStreamResult {
        target_names,
        captured_chunks,
        silence_chunks,
        dropped_chunks,
        elapsed: started.elapsed(),
    })
}
