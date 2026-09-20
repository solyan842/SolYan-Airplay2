use airplay2_audio::{LiveAudioDecoder, LivePcmFrame};
use airplay2_client::AirPlayClient;
use airplay2_core::{Device, StreamConfig};
use airplay2_core::error::{Error as AirPlayError, RtspError as AirPlayRtspError};
use anyhow::{anyhow, bail, Result};
use audio_capture::{start_default_loopback, AudioFormat as CaptureFormat, CaptureHandle};
use crossbeam_channel::Sender;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex as AsyncMutex;

const MAX_SYNTHETIC_QUEUE_CHUNKS: usize = 4;

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
    pub signal_chunks: u64,
    pub signal_peak: u16,
    pub signal_confirmed: bool,
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
    pub capture_restarts: u64,
    pub feedback_timeout_streak: u32,
    pub control_degraded: bool,
}

#[derive(Debug, Clone)]
pub struct LiveStreamResult {
    pub target_names: Vec<String>,
    pub captured_chunks: u64,
    pub silence_chunks: u64,
    pub late_polls: u64,
    pub silence_transitions: u64,
    pub dropped_chunks: u64,
    pub capture_restarts: u64,
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

fn os_major(version: Option<&str>) -> Option<u32> {
    version?
        .split(|ch: char| !ch.is_ascii_digit())
        .find(|part| !part.is_empty())?
        .parse()
        .ok()
}

fn should_follow_receiver_clock(device: &Device) -> bool {
    let is_homepod = device
        .model
        .to_ascii_lowercase()
        .starts_with("audioaccessory");
    let os27_or_newer = os_major(device.os_version.as_deref())
        .map(|major| major >= 27)
        .unwrap_or(false);

    is_homepod
        && os27_or_newer
        && device.is_group_leader
        && device.parent_group_id.is_none()
        && device.tight_sync_id.is_none()
}

fn validate_capture_format(capture: &CaptureHandle, expected_rate: Option<u32>) -> Result<()> {
    if capture.format.channels != 2 || capture.format.bits_per_sample != 16 {
        bail!(
            "unexpected normalized WASAPI format: {} Hz / {} ch / {} bit",
            capture.format.sample_rate,
            capture.format.channels,
            capture.format.bits_per_sample
        );
    }

    if let Some(rate) = expected_rate {
        if capture.format.sample_rate != rate {
            bail!(
                "WASAPI mix format changed during playback: {} -> {} Hz",
                rate,
                capture.format.sample_rate
            );
        }
    }

    Ok(())
}

async fn reopen_default_loopback(
    expected_rate: u32,
    control: &StreamControl,
    attempts: usize,
) -> Result<CaptureHandle> {
    let mut last_error = None;

    for attempt in 1..=attempts {
        if control.is_stopped() {
            bail!("capture recovery cancelled");
        }

        match start_default_loopback(CaptureFormat::default()) {
            Ok(capture) => match validate_capture_format(&capture, Some(expected_rate)) {
                Ok(()) => {
                    tracing::info!(
                        "WASAPI capture recovered on attempt {}: {} @ {} Hz",
                        attempt,
                        capture.device_name,
                        capture.format.sample_rate
                    );
                    return Ok(capture);
                }
                Err(err) => {
                    last_error = Some(err.to_string());
                }
            },
            Err(err) => {
                last_error = Some(err.to_string());
            }
        }

        tracing::warn!(
            "WASAPI reopen attempt {}/{} failed: {}",
            attempt,
            attempts,
            last_error.as_deref().unwrap_or("unknown error")
        );

        tokio::time::sleep(Duration::from_millis(
            250 * attempt.min(4) as u64
        ))
        .await;
    }

    bail!(
        "WASAPI recovery exhausted after {} attempts: {}",
        attempts,
        last_error.unwrap_or_else(|| "unknown error".to_string())
    )
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
    // Keep the live handoff shallow enough to stay real-time while still
    // carrying enough runway for the streamer's ~400ms internal prime.
    const LIVE_QUEUE_CHUNKS: usize = 32;
    const SILENCE_POLL: Duration = Duration::from_millis(8);
    // 1024 frames at a common 48kHz Windows endpoint arrive every ~21.3ms.
    // A 20ms receive timeout classified healthy cadence as "late".
    const ACTIVE_POLL: Duration = Duration::from_millis(35);
    // The AirPlay streamer already has hundreds of milliseconds of PCM runway.
    // Do not fade to synthetic silence for ordinary scheduler hiccups.
    const SILENCE_GRACE: Duration = Duration::from_millis(350);
    const STARTUP_PRIME_MS: u64 = 450;
    const FEEDBACK_DEGRADED_MISSES: u32 = 3;
    const CAPTURE_RESTART_ATTEMPTS: usize = 6;
    const RTP_STALL_LIMIT: Duration = Duration::from_secs(3);
    const TRANSITION_MS: u32 = 2;
    const SIGNAL_PEAK_THRESHOLD: u16 = 32;
    const SIGNAL_CONFIRM_PACKETS: u64 = 120;

    let mut stream_config = StreamConfig::default();
    if render_delay_ms == 0 {
        // Video profile: ask for the HomePod-oriented low-latency window.
        // 3087 samples ≈ 70ms at 44.1kHz, matching upstream HomePod
        // arrivalToRenderLatency observations. Receivers may clamp/ignore this.
        stream_config.latency_min = 3_087;
        stream_config.latency_max = 11_025; // ≈250ms
    }

    // Discovery must happen before final timing-route selection because
    // standalone HomePod OS 27 follows a different gPTP clock ownership model.
    let discovery_client = AirPlayClient::new()?;
    let targets =
        discover_targets(&discovery_client, &selectors, mode, Duration::from_secs(4)).await?;
    let target_names = targets.iter().map(|d| d.name.clone()).collect::<Vec<_>>();

    if mode == LiveStreamMode::Single && should_follow_receiver_clock(&targets[0]) {
        stream_config.ptp_mode = airplay2_core::PtpMode::Slave;
        tracing::warn!(
            "Timing route: FOLLOW RECEIVER CLOCK — name={}, model={}, osvers={}, igl={}, pgid={:?}, tsid={:?}",
            targets[0].name,
            targets[0].model,
            targets[0].os_version.as_deref().unwrap_or("unknown"),
            targets[0].is_group_leader,
            targets[0].parent_group_id,
            targets[0].tight_sync_id
        );
    } else if mode == LiveStreamMode::Single {
        stream_config.ptp_mode = airplay2_core::PtpMode::Master;
        tracing::info!(
            "Timing route: SENDER GRANDMASTER — name={}, model={}, osvers={}, igl={}, pgid={:?}, tsid={:?}",
            targets[0].name,
            targets[0].model,
            targets[0].os_version.as_deref().unwrap_or("unknown"),
            targets[0].is_group_leader,
            targets[0].parent_group_id,
            targets[0].tight_sync_id
        );
    }

    let mut client = AirPlayClient::with_config(stream_config, None)?;
    client.set_render_delay_ms(render_delay_ms);

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

    // Latch the requested volume as soon as the RTSP session exists. Some
    // receivers can briefly restore their previous hardware/session volume while
    // RECORD/startup is still settling, so one post-start SET_PARAMETER is not
    // sufficient on every launch.
    let initial_volume = control.volume();
    client.set_volume(initial_volume).await?;

    let mut capture = start_default_loopback(CaptureFormat::default())?;
    if let Err(err) = validate_capture_format(&capture, None) {
        let _ = client.disconnect().await;
        return Err(err);
    }

    let source_rate = capture.format.sample_rate;
    let source_block_frames: usize = 1024;
    let transition_frames =
        ((source_rate as u64 * TRANSITION_MS as u64) / 1000).max(1) as usize;
    let silence_chunk_period =
        Duration::from_secs_f64(source_block_frames as f64 / source_rate as f64);

    tracing::info!(
        "Live source format: {} Hz stereo i16 -> AirPlay {} Hz / {} frames per packet",
        source_rate,
        TARGET_RATE,
        TARGET_PACKET_FRAMES
    );

    let (sender, decoder) =
        LiveAudioDecoder::create_pair(source_rate, CHANNELS, LIVE_QUEUE_CHUNKS);

    // Native AP2 senders arm the session first, then start only when audio is
    // already buffered. Prime from the actual Windows loopback stream instead
    // of inserting a fixed half-second of silence in front of the music.
    //
    // If the Windows render endpoint is genuinely idle, feed silence at SOURCE
    // cadence so the priming duration remains bounded and the RTP timeline can
    // still start cleanly when content arrives later.
    let startup_prime_chunks = (
        (source_rate as u64 * STARTUP_PRIME_MS / 1000)
            + source_block_frames as u64
            - 1
    ) / source_block_frames as u64;

    let mut startup_chunks = 0u64;
    let mut captured_chunks = 0u64;
    let mut signal_chunks = 0u64;
    let mut signal_peak = 0u16;
    let mut silence_chunks = 0u64;
    let mut late_polls = 0u64;
    let mut silence_transitions = 0u64;
    let mut in_silence = true;
    let mut last_real_at: Option<Instant> = None;
    let mut last_samples = vec![0i16; CHANNELS as usize];
    let mut next_prime_silence_at = Instant::now();

    while startup_chunks < startup_prime_chunks && !control.is_stopped() {
        match capture.poll_timeout(SILENCE_POLL)? {
            Some(chunk) => {
                let mut samples = chunk.samples;
                let chunk_peak = samples
                    .iter()
                    .map(|sample| sample.unsigned_abs())
                    .max()
                    .unwrap_or(0);

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

                if sender.send(LivePcmFrame {
                    samples,
                    channels: chunk.channels as u8,
                    sample_rate: chunk.sample_rate,
                }) {
                    startup_chunks += 1;
                    captured_chunks += 1;
                    let now = Instant::now();
                    last_real_at = Some(now);
                    next_prime_silence_at = now + silence_chunk_period;
                    if chunk_peak > SIGNAL_PEAK_THRESHOLD {
                        signal_chunks += 1;
                        signal_peak = signal_peak.max(chunk_peak);
                    }
                }
            }
            None => {
                let now = Instant::now();
                if now >= next_prime_silence_at {
                    if sender.send(LivePcmFrame {
                        samples: vec![0; source_block_frames * CHANNELS as usize],
                        channels: CHANNELS,
                        sample_rate: source_rate,
                    }) {
                        startup_chunks += 1;
                        silence_chunks += 1;
                        if !in_silence {
                            in_silence = true;
                            silence_transitions += 1;
                            last_samples.fill(0);
                        }
                    }
                    next_prime_silence_at = now + silence_chunk_period;
                }
            }
        }
    }

    tracing::info!(
        "Startup PCM prime ready: chunks={}, source_ms≈{}, captured={}, synthetic_silence={}, signal={}",
        startup_chunks,
        STARTUP_PRIME_MS,
        captured_chunks,
        silence_chunks,
        signal_chunks
    );

    if control.is_stopped() {
        capture.stop();
        let _ = client.disconnect().await;
        bail!("stream cancelled before playback started");
    }

    match mode {
        LiveStreamMode::Single => {
            client.start_live_streaming_with_decoder(decoder).await?;
        }
        LiveStreamMode::MultiroomExperimental => {
            // Group startup already performs FLUSH -> RECORD for every member.
            client.start_live_streaming_to_group(decoder).await?
        }
    }

    // From the instant RTP starts, no RTSP operation may block PCM delivery.
    // Feedback and the two defensive startup-volume reassertions run on the
    // control plane behind their own async mutex.
    let mut applied_volume = initial_volume;
    let client = Arc::new(AsyncMutex::new(client));

    let feedback_client = Arc::clone(&client);
    let feedback_stop = Arc::new(AtomicBool::new(false));
    let feedback_stop_worker = Arc::clone(&feedback_stop);
    let feedback_fatal = Arc::new(AtomicBool::new(false));
    let feedback_fatal_worker = Arc::clone(&feedback_fatal);
    let feedback_timeout_streak = Arc::new(AtomicU32::new(0));
    let feedback_timeout_streak_worker = Arc::clone(&feedback_timeout_streak);
    let feedback_degraded = Arc::new(AtomicBool::new(false));
    let feedback_degraded_worker = Arc::clone(&feedback_degraded);
    let feedback_task = tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(2));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        ticker.tick().await; // consume the immediate first tick
        let mut consecutive_misses = 0u32;

        loop {
            ticker.tick().await;
            if feedback_stop_worker.load(Ordering::Acquire) {
                break;
            }

            let feedback_started = Instant::now();
            let result = {
                let mut guard = feedback_client.lock().await;
                guard.send_feedback().await
            };
            let elapsed = feedback_started.elapsed();

            match result {
                Ok(()) => {
                    if consecutive_misses > 0 {
                        tracing::info!(
                            "AirPlay feedback recovered after {} timeout(s)",
                            consecutive_misses
                        );
                    }
                    consecutive_misses = 0;
                    feedback_timeout_streak_worker.store(0, Ordering::Release);
                    feedback_degraded_worker.store(false, Ordering::Release);
                }
                Err(AirPlayError::Timeout)
                | Err(AirPlayError::Rtsp(AirPlayRtspError::UnexpectedStatus(_))) => {
                    consecutive_misses += 1;
                    feedback_timeout_streak_worker
                        .store(consecutive_misses, Ordering::Release);

                    if consecutive_misses >= FEEDBACK_DEGRADED_MISSES {
                        feedback_degraded_worker.store(true, Ordering::Release);
                        tracing::warn!(
                            "AirPlay feedback degraded: {} consecutive keepalive misses; RTP continues",
                            consecutive_misses
                        );
                    } else {
                        tracing::warn!(
                            "AirPlay feedback miss {}/{}; RTP continues",
                            consecutive_misses,
                            FEEDBACK_DEGRADED_MISSES
                        );
                    }
                }
                Err(err) => {
                    // Transport/framing errors mean the RTSP channel itself is
                    // no longer trustworthy. Unlike an HTTP status miss, this
                    // is a real control-plane failure.
                    tracing::error!("AirPlay control channel transport failure: {err}");
                    feedback_fatal_worker.store(true, Ordering::Release);
                    break;
                }
            }

            if elapsed >= Duration::from_millis(500) {
                tracing::warn!(
                    "AirPlay feedback was slow ({:.0} ms); PCM delivery stayed independent",
                    elapsed.as_secs_f64() * 1000.0
                );
            }
        }
    });

    let volume_client = Arc::clone(&client);
    let volume_control = control.clone();
    let volume_task = tokio::spawn(async move {
        for delay in [150u64, 350u64] {
            tokio::time::sleep(Duration::from_millis(delay)).await;
            if volume_control.is_stopped() {
                break;
            }
            let desired = volume_control.volume();
            let _ = tokio::time::timeout(Duration::from_millis(750), async {
                let mut guard = volume_client.lock().await;
                guard.set_volume(desired).await
            })
            .await;
        }
    });

    let started = Instant::now();
    let mut first_signal_packet_baseline: Option<u64> =
        if signal_chunks > 0 { Some(0) } else { None };
    let mut signal_confirmed = false;
    let mut dropped_chunks = 0u64;
    let mut next_progress = Instant::now() + Duration::from_millis(500);
    let mut next_health_log = Instant::now() + Duration::from_secs(1);
    let mut next_silence_at = Instant::now() + silence_chunk_period;
    let mut packets_sent = 0u64;
    let mut last_packet_observed = 0u64;
    let mut packet_progress_seen = false;
    let mut last_packet_progress_at = Instant::now();
    let mut retransmit_requested = 0u64;
    let mut retransmit_fulfilled = 0u64;
    let mut underruns = 0u64;
    let mut loss_percent = 0.0f64;
    let mut capture_restarts = 0u64;
    let mut loop_error: Option<anyhow::Error> = None;

    while !control.is_stopped() {
        if feedback_fatal.load(Ordering::Acquire) {
            loop_error = Some(anyhow!("AirPlay RTSP control channel failed"));
            break;
        }

        let poll_window = if in_silence { SILENCE_POLL } else { ACTIVE_POLL };
        let captured = match capture.poll_timeout(poll_window) {
            Ok(captured) => captured,
            Err(err) => {
                if control.is_stopped() {
                    break;
                }

                tracing::warn!(
                    "WASAPI capture interrupted: {}. Keeping AirPlay RTP alive while reopening the default render endpoint.",
                    err
                );

                capture.stop();

                match reopen_default_loopback(
                    source_rate,
                    &control,
                    CAPTURE_RESTART_ATTEMPTS,
                )
                .await
                {
                    Ok(new_capture) => {
                        capture = new_capture;
                        capture_restarts += 1;
                        in_silence = true;
                        last_real_at = None;
                        last_samples.fill(0);
                        next_silence_at = Instant::now() + silence_chunk_period;
                        tracing::info!(
                            "WASAPI recovery complete; AirPlay session preserved (restart #{})",
                            capture_restarts
                        );
                        continue;
                    }
                    Err(recovery_error) => {
                        loop_error = Some(anyhow!(
                            "WASAPI capture failed and could not recover: {}; recovery: {}",
                            err,
                            recovery_error
                        ));
                        break;
                    }
                }
            }
        };

        match captured {
            Some(chunk) => {
                let mut samples = chunk.samples;
                let chunk_peak = samples
                    .iter()
                    .map(|sample| sample.unsigned_abs())
                    .max()
                    .unwrap_or(0);

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
                    if chunk_peak > SIGNAL_PEAK_THRESHOLD {
                        signal_chunks += 1;
                        signal_peak = signal_peak.max(chunk_peak);
                        if first_signal_packet_baseline.is_none() {
                            if let Ok(guard) = client.try_lock() {
                                packets_sent = guard.stats_snapshot().packets_sent;
                            }
                            first_signal_packet_baseline = Some(packets_sent);
                            tracing::info!(
                                "First non-silent PCM queued: peak={}, packet_baseline={}",
                                chunk_peak,
                                packets_sent
                            );
                        }
                    }
                    last_real_at = Some(Instant::now());
                } else {
                    dropped_chunks += 1;
                }
            }
            None => {
                late_polls += 1;
                let now = Instant::now();

                if !in_silence {
                    let grace_elapsed = last_real_at
                        .map(|t| t.elapsed() >= SILENCE_GRACE)
                        .unwrap_or(false);

                    // Short WASAPI scheduling gaps are absorbed by the audio buffer.
                    // Do not manufacture audio during the grace window, but still
                    // run volume/progress logic below.
                    if grace_elapsed {
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

                        // Synthetic audio must never block the real PCM producer.
                        if sender.queued_frames() < MAX_SYNTHETIC_QUEUE_CHUNKS
                            && sender.try_send(frame)
                        {
                            silence_chunks += 1;
                            silence_transitions += 1;
                            in_silence = true;
                            last_samples.fill(0);
                            next_silence_at = now + silence_chunk_period;
                        }
                    }
                } else if now >= next_silence_at {
                    // Keep the wire alive at the SOURCE AUDIO cadence. Polling is
                    // intentionally faster than a PCM block, so emitting one silence
                    // block per poll would overfill the queue and bury the next track.
                    if sender.queued_frames() < MAX_SYNTHETIC_QUEUE_CHUNKS {
                        let frame = LivePcmFrame {
                            samples: vec![0; source_block_frames * CHANNELS as usize],
                            channels: CHANNELS,
                            sample_rate: source_rate,
                        };

                        if sender.try_send(frame) {
                            silence_chunks += 1;
                        }
                    }

                    // Never catch up missed silence in a burst.
                    next_silence_at = now + silence_chunk_period;
                }
            }
        }


        let desired_volume = control.volume();
        if (desired_volume - applied_volume).abs() > 0.005 {
            // Never wait behind feedback on the RTSP control plane. If the
            // control mutex is busy, retry on the next producer iteration.
            if let Ok(mut guard) = client.try_lock() {
                if guard.set_volume(desired_volume).await.is_ok() {
                    applied_volume = desired_volume;
                }
            }
        }

        let now = Instant::now();
        if now >= next_progress {
            // Stats are observational; stale values are safer than blocking PCM.
            let mut stats_fresh = false;
            if let Ok(guard) = client.try_lock() {
                let stats = guard.stats_snapshot();
                packets_sent = stats.packets_sent;
                retransmit_requested = stats.rtx_requested;
                retransmit_fulfilled = stats.rtx_fulfilled;
                underruns = stats.underruns;
                loss_percent = stats.loss_percent();
                stats_fresh = true;

                if !packet_progress_seen {
                    if packets_sent > 0 {
                        packet_progress_seen = true;
                        last_packet_observed = packets_sent;
                        last_packet_progress_at = now;
                    }
                } else if packets_sent > last_packet_observed {
                    last_packet_observed = packets_sent;
                    last_packet_progress_at = now;
                } else if last_packet_progress_at.elapsed() >= RTP_STALL_LIMIT {
                    tracing::error!(
                        "RTP HEARTBEAT STALLED: packets_sent={} unchanged for {:.1}s; captured={}, silence={}, queue={}, feedback_streak={}",
                        packets_sent,
                        last_packet_progress_at.elapsed().as_secs_f64(),
                        captured_chunks,
                        silence_chunks,
                        sender.queued_frames(),
                        feedback_timeout_streak.load(Ordering::Acquire)
                    );
                    loop_error = Some(anyhow!(
                        "RTP packet clock stalled at {} packets for {:.1}s",
                        packets_sent,
                        last_packet_progress_at.elapsed().as_secs_f64()
                    ));
                    break;
                }
            }

            if now >= next_health_log {
                tracing::info!(
                    "SERVICE HEALTH: captured={}, silence={}, queue={}, packets_sent={}, stats_fresh={}, feedback_streak={}, control_degraded={}, capture_restarts={}",
                    captured_chunks,
                    silence_chunks,
                    sender.queued_frames(),
                    packets_sent,
                    stats_fresh,
                    feedback_timeout_streak.load(Ordering::Acquire),
                    feedback_degraded.load(Ordering::Acquire),
                    capture_restarts
                );
                next_health_log = now + Duration::from_secs(1);
            }

            if !signal_confirmed {
                if let Some(baseline) = first_signal_packet_baseline {
                    if packets_sent.saturating_sub(baseline) >= SIGNAL_CONFIRM_PACKETS {
                        signal_confirmed = true;
                        tracing::info!(
                            "Non-silent PCM has cleared startup prebuffer/render lead: packets_advanced={}",
                            packets_sent.saturating_sub(baseline)
                        );
                    }
                }
            }

            if let Some(tx) = &progress_tx {
                let _ = tx.try_send(StreamProgress {
                    elapsed_secs: started.elapsed().as_secs_f64(),
                    captured_chunks,
                    signal_chunks,
                    signal_peak,
                    signal_confirmed,
                    silence_chunks,
                    late_polls,
                    silence_transitions,
                    dropped_chunks,
                    packets_sent,
                    retransmit_requested,
                    retransmit_fulfilled,
                    underruns,
                    loss_percent,
                    drift_ppm: 0.0,
                    target_count: targets.len(),
                    capture_restarts,
                    feedback_timeout_streak: feedback_timeout_streak.load(Ordering::Acquire),
                    control_degraded: feedback_degraded.load(Ordering::Acquire),
                });
            }
            next_progress = now + Duration::from_millis(500);
        }
    }

    capture.stop();
    drop(sender);

    // Stop control-plane work before owning the client for shutdown.
    feedback_stop.store(true, Ordering::Release);
    feedback_task.abort();
    volume_task.abort();
    let _ = feedback_task.await;
    let _ = volume_task.await;

    {
        let mut guard = client.lock().await;
        let _ = guard.stop().await;
        let _ = guard.disconnect().await;
    }

    if let Some(err) = loop_error {
        return Err(err);
    }

    Ok(LiveStreamResult {
        target_names,
        captured_chunks,
        silence_chunks,
        late_polls,
        silence_transitions,
        dropped_chunks,
        capture_restarts,
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
    fn synthetic_silence_is_paced_at_source_audio_time() {
        let source_rate = 48_000u32;
        let source_block_frames = 1024usize;
        let period =
            Duration::from_secs_f64(source_block_frames as f64 / source_rate as f64);

        assert!(period >= Duration::from_millis(21));
        assert!(period <= Duration::from_millis(22));
        assert!(MAX_SYNTHETIC_QUEUE_CHUNKS <= 4);
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
