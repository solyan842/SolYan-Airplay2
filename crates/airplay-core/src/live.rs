use airplay2_audio::{LiveAudioDecoder, LivePcmFrame};
use airplay2_client::AirPlayClient;
use airplay2_core::{Device, StreamConfig};
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
    pub late_polls: u64,
    pub silence_transitions: u64,
    pub dropped_chunks: u64,
    pub packets_sent: u64,
    pub retransmit_requested: u64,
    pub retransmit_fulfilled: u64,
    pub underruns: u64,
    pub loss_percent: f64,
    pub drift_ppm: f64,
    pub target_count: usize,
}

#[derive(Debug, Clone)]
pub struct LiveStreamResult {
    pub target_names: Vec<String>,
    pub captured_chunks: u64,
    pub silence_chunks: u64,
    pub late_polls: u64,
    pub silence_transitions: u64,
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
    const TARGET_RATE: u32 = 44_100;
    const CHANNELS: u8 = 2;
    const TARGET_PACKET_FRAMES: usize = 352;
    const PREBUFFER_CHUNKS: u64 = 24;
    const LIVE_QUEUE_CHUNKS: usize = 96;
    const SILENCE_POLL: Duration = Duration::from_millis(8);
    const ACTIVE_POLL: Duration = Duration::from_millis(20);
    const SILENCE_GRACE: Duration = Duration::from_millis(120);
    const TRANSITION_MS: u32 = 2;

    let mut stream_config = StreamConfig::default();
    if render_delay_ms == 0 {
        // Video profile: ask for the HomePod-oriented low-latency window.
        // 3087 samples ≈ 70ms at 44.1kHz, matching upstream HomePod
        // arrivalToRenderLatency observations. Receivers may clamp/ignore this.
        stream_config.latency_min = 3_087;
        stream_config.latency_max = 11_025; // ≈250ms
    }

    let mut client = AirPlayClient::with_config(stream_config, None)?;
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
    if capture.format.channels != CHANNELS as u16 || capture.format.bits_per_sample != 16 {
        let _ = client.disconnect().await;
        bail!(
            "unexpected normalized WASAPI format: {} Hz / {} ch / {} bit",
            capture.format.sample_rate,
            capture.format.channels,
            capture.format.bits_per_sample
        );
    }

    let source_rate = capture.format.sample_rate;
    let source_block_frames: usize = 1024;
    let transition_frames =
        ((source_rate as u64 * TRANSITION_MS as u64) / 1000).max(1) as usize;

    tracing::info!(
        "Live source format: {} Hz stereo i16 -> AirPlay {} Hz / {} frames per packet",
        source_rate,
        TARGET_RATE,
        TARGET_PACKET_FRAMES
    );

    let (sender, decoder) =
        LiveAudioDecoder::create_pair(source_rate, CHANNELS, LIVE_QUEUE_CHUNKS);

    // Prime AirPlay with pure silence only. Mixing "maybe real / maybe silence"
    // during session startup can create a discontinuity exactly when RECORD starts.
    // The first real PCM block is faded in after streaming is established.
    let mut prebuffered = 0u64;
    let mut silence_chunks = 0u64;
    while prebuffered < PREBUFFER_CHUNKS && !control.is_stopped() {
        let frame = LivePcmFrame {
            samples: vec![0; source_block_frames * CHANNELS as usize],
            channels: CHANNELS,
            sample_rate: source_rate,
        };
        if sender.send(frame) {
            prebuffered += 1;
            silence_chunks += 1;
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
    let mut late_polls = 0u64;
    let mut silence_transitions = 0u64;
    let mut in_silence = true;
    let mut last_real_at: Option<Instant> = None;
    let mut last_samples = vec![0i16; CHANNELS as usize];
    let mut next_feedback = Instant::now() + Duration::from_secs(2);
    let mut next_progress = Instant::now() + Duration::from_millis(500);

    // Measure the physical WASAPI endpoint clock against the same monotonic
    // timebase used by the RTP sender. A small ppm mismatch accumulates over
    // time and can otherwise cause occasional discontinuities/clicks.
    let mut drift_window_start = Instant::now();
    let mut drift_window_frames = capture.total_frames();
    let mut drift_ppm_ema = 0.0f64;
    let mut next_drift_update = Instant::now() + Duration::from_secs(5);

    while !control.is_stopped() {
        let poll_window = if in_silence { SILENCE_POLL } else { ACTIVE_POLL };

        match capture.poll_timeout(poll_window)? {
            Some(chunk) => {
                let mut samples = chunk.samples;

                if in_silence {
                    fade_in_from_zero(
                        &mut samples,
                        chunk.channels as usize,
                        transition_frames,
                    );
                    in_silence = false;
                    silence_transitions += 1;
                }

                remember_last_samples(
                    &samples,
                    chunk.channels as usize,
                    &mut last_samples,
                );

                let frame = LivePcmFrame {
                    samples,
                    channels: chunk.channels as u8,
                    sample_rate: chunk.sample_rate,
                };

                if sender.send(frame) {
                    captured_chunks += 1;
                    last_real_at = Some(Instant::now());
                } else {
                    dropped_chunks += 1;
                }
            }
            None => {
                late_polls += 1;

                if !in_silence {
                    let grace_elapsed = last_real_at
                        .map(|t| t.elapsed() >= SILENCE_GRACE)
                        .unwrap_or(false);

                    // A short WASAPI scheduling gap is normal. Do NOT inject a zero
                    // packet here: the 400-500ms audio buffer is specifically there
                    // to absorb this jitter. Inserting zero after one late poll was
                    // the source of intermittent clicks in v0.1.5.
                    if !grace_elapsed {
                        continue;
                    }

                    let samples = ramp_to_zero(
                        &last_samples,
                        CHANNELS as usize,
                        source_block_frames,
                        transition_frames,
                    );
                    let frame = LivePcmFrame {
                        samples,
                        channels: CHANNELS,
                        sample_rate: source_rate,
                    };

                    if sender.send(frame) {
                        silence_chunks += 1;
                        silence_transitions += 1;
                        in_silence = true;
                        last_samples.fill(0);
                    } else {
                        dropped_chunks += 1;
                    }
                } else {
                    // Once truly silent, keep the AirPlay pipeline clocked at the
                    // packet cadence. Real PCM will replace silence immediately and
                    // receives a short fade-in on the transition back.
                    let frame = LivePcmFrame {
                        samples: vec![0; source_block_frames * CHANNELS as usize],
                        channels: CHANNELS,
                        sample_rate: source_rate,
                    };

                    if sender.send(frame) {
                        silence_chunks += 1;
                    } else {
                        dropped_chunks += 1;
                    }
                }
            }
        }

        let now = Instant::now();
        if now >= next_drift_update {
            let total_frames = capture.total_frames();
            let delta_frames = total_frames.saturating_sub(drift_window_frames);
            let dt = now.duration_since(drift_window_start).as_secs_f64();

            // Ignore windows where the endpoint was effectively idle. Clock-rate
            // estimation is meaningful only when enough hardware frames arrived.
            if !in_silence && dt >= 4.0 && delta_frames >= source_rate as u64 * 2 {
                let measured_rate = delta_frames as f64 / dt;
                let source_error_ppm =
                    (measured_rate / source_rate as f64 - 1.0) * 1_000_000.0;

                // If the source hardware runs fast, lower output/input ratio so
                // more source frames are consumed per AirPlay output frame.
                let desired_correction = (-source_error_ppm).clamp(-500.0, 500.0);

                // Slow EMA prevents short scheduler noise from modulating pitch.
                drift_ppm_ema =
                    (drift_ppm_ema * 0.80 + desired_correction * 0.20)
                        .clamp(-500.0, 500.0);
                sender.set_drift_ppm(drift_ppm_ema);

                tracing::info!(
                    "Clock drift: nominal={}Hz measured={:.3}Hz source_error={:+.1}ppm correction={:+.1}ppm",
                    source_rate,
                    measured_rate,
                    source_error_ppm,
                    drift_ppm_ema
                );
            }

            drift_window_start = now;
            drift_window_frames = total_frames;
            next_drift_update = now + Duration::from_secs(5);
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
                    late_polls,
                    silence_transitions,
                    dropped_chunks,
                    packets_sent: stats.packets_sent,
                    retransmit_requested: stats.rtx_requested,
                    retransmit_fulfilled: stats.rtx_fulfilled,
                    underruns: stats.underruns,
                    loss_percent: stats.loss_percent(),
                    drift_ppm: sender.drift_ppm(),
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
        late_polls,
        silence_transitions,
        dropped_chunks,
        elapsed: started.elapsed(),
    })
}


fn fade_in_from_zero(samples: &mut [i16], channels: usize, transition_frames: usize) {
    if channels == 0 {
        return;
    }
    let frames = (samples.len() / channels).min(transition_frames).max(1);
    for frame in 0..frames {
        let gain = frame as f32 / frames as f32;
        for ch in 0..channels {
            let idx = frame * channels + ch;
            samples[idx] = (samples[idx] as f32 * gain).round() as i16;
        }
    }
}

fn remember_last_samples(samples: &[i16], channels: usize, out: &mut [i16]) {
    if channels == 0 || samples.len() < channels {
        return;
    }
    let start = samples.len() - channels;
    for ch in 0..channels.min(out.len()) {
        out[ch] = samples[start + ch];
    }
}

fn ramp_to_zero(
    last_samples: &[i16],
    channels: usize,
    chunk_frames: usize,
    transition_frames: usize,
) -> Vec<i16> {
    let mut out = vec![0i16; chunk_frames * channels];
    if channels == 0 {
        return out;
    }

    let frames = chunk_frames.min(transition_frames).max(1);
    for frame in 0..frames {
        let gain = 1.0 - (frame as f32 / frames as f32);
        for ch in 0..channels {
            let source = *last_samples.get(ch).unwrap_or(&0);
            out[frame * channels + ch] = (source as f32 * gain).round() as i16;
        }
    }
    out
}

#[cfg(test)]
mod clickless_tests {
    use super::*;

    #[test]
    fn fade_in_starts_near_zero_and_reaches_signal() {
        let mut samples = vec![10_000i16; 352 * 2];
        fade_in_from_zero(&mut samples, 2, 96);
        assert_eq!(samples[0], 0);
        assert!(samples[95 * 2] > 9_000);
        assert_eq!(samples[200 * 2], 10_000);
    }

    #[test]
    fn ramp_to_zero_preserves_channel_shape_without_hard_step() {
        let samples = ramp_to_zero(&[12_000, -8_000], 2, 352, 96);
        assert_eq!(samples[0], 12_000);
        assert_eq!(samples[1], -8_000);
        assert!(samples[95 * 2].abs() < 500);
        assert_eq!(samples[120 * 2], 0);
        assert_eq!(samples[120 * 2 + 1], 0);
    }
}
