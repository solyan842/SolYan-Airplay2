use airplay2_audio::{LiveAudioDecoder, LivePcmFrame};
use airplay2_client::AirPlayClient;
use airplay2_core::Device;
use anyhow::{anyhow, bail, Result};
use audio_capture::{start_default_loopback, AudioFormat as CaptureFormat};
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct SessionTestResult {
    pub name: String,
    pub model: String,
    pub address: String,
    pub supports_airplay2: bool,
    pub supports_ptp: bool,
}

#[derive(Debug, Clone)]
pub struct StreamTestResult {
    pub session: SessionTestResult,
    pub prebuffer_chunks: u64,
    pub sent_chunks: u64,
    pub dropped_chunks: u64,
    pub stream_duration: Duration,
}

async fn select_audio_device(
    client: &AirPlayClient,
    selector: Option<&str>,
    discovery_timeout: Duration,
) -> Result<Device> {
    let mut devices = client.discover(discovery_timeout).await?;
    devices.retain(|d| d.features.supports_audio());

    if devices.is_empty() {
        bail!("no AirPlay audio receivers found");
    }

    if let Some(selector) = selector {
        let needle = selector.to_lowercase();
        return devices
            .into_iter()
            .find(|d| {
                d.name.to_lowercase().contains(&needle)
                    || d.model.to_lowercase().contains(&needle)
                    || d.addresses.iter().any(|ip| ip.to_string() == selector)
            })
            .ok_or_else(|| anyhow!("no AirPlay audio receiver matched '{selector}'"));
    }

    if devices.len() == 1 {
        return Ok(devices.remove(0));
    }

    let homepod_matches: Vec<_> = devices
        .iter()
        .filter(|d| {
            let model = d.model.to_lowercase();
            let name = d.name.to_lowercase();
            model.contains("audioaccessory") || name.contains("homepod")
        })
        .cloned()
        .collect();

    if homepod_matches.len() == 1 {
        return Ok(homepod_matches.into_iter().next().unwrap());
    }

    let choices = devices
        .iter()
        .map(|d| format!("{} ({})", d.name, d.model))
        .collect::<Vec<_>>()
        .join(", ");
    bail!("multiple AirPlay audio receivers found; specify a name or IP. Choices: {choices}");
}

fn session_result(device: &Device) -> SessionTestResult {
    let address = device
        .addresses
        .iter()
        .find(|ip| ip.is_ipv4())
        .or_else(|| device.addresses.first())
        .map(ToString::to_string)
        .unwrap_or_else(|| "<unknown>".to_string());

    SessionTestResult {
        name: device.name.clone(),
        model: device.model.clone(),
        address,
        supports_airplay2: device.supports_airplay2(),
        supports_ptp: device.supports_ptp(),
    }
}

pub async fn connect_test(
    selector: Option<&str>,
    discovery_timeout: Duration,
    connect_timeout: Duration,
) -> Result<SessionTestResult> {
    let mut client = AirPlayClient::new()?;
    client.set_render_delay_ms(200);

    let selected = select_audio_device(&client, selector, discovery_timeout).await?;
    let result = session_result(&selected);

    tokio::time::timeout(connect_timeout, client.connect(&selected))
        .await
        .map_err(|_| anyhow!("AirPlay connect/setup timed out after {connect_timeout:?}"))??;

    tokio::time::timeout(Duration::from_secs(5), client.disconnect())
        .await
        .map_err(|_| anyhow!("AirPlay TEARDOWN timed out"))??;

    Ok(result)
}

pub async fn stream_test(
    selector: Option<&str>,
    discovery_timeout: Duration,
    connect_timeout: Duration,
    stream_duration: Duration,
) -> Result<StreamTestResult> {
    const SAMPLE_RATE: u32 = 44_100;
    const CHANNELS: u8 = 2;
    const PREBUFFER_CHUNKS: u64 = 25; // ~200ms at 352 frames/chunk.
    const LIVE_QUEUE_CHUNKS: usize = 64;

    let mut client = AirPlayClient::new()?;
    client.set_render_delay_ms(200);

    let selected = select_audio_device(&client, selector, discovery_timeout).await?;
    let session = session_result(&selected);

    tokio::time::timeout(connect_timeout, client.connect(&selected))
        .await
        .map_err(|_| anyhow!("AirPlay connect/setup timed out after {connect_timeout:?}"))??;

    let mut capture = start_default_loopback(CaptureFormat::default())?;
    if capture.format.sample_rate != SAMPLE_RATE
        || capture.format.channels != CHANNELS as u16
        || capture.format.bits_per_sample != 16
    {
        bail!(
            "unexpected WASAPI format: {} Hz / {} ch / {} bit",
            capture.format.sample_rate,
            capture.format.channels,
            capture.format.bits_per_sample
        );
    }

    let (sender, decoder) =
        LiveAudioDecoder::create_pair(SAMPLE_RATE, CHANNELS, LIVE_QUEUE_CHUNKS);

    let prebuffer_deadline = Instant::now() + Duration::from_secs(5);
    let mut prebuffered = 0u64;
    while prebuffered < PREBUFFER_CHUNKS {
        if Instant::now() >= prebuffer_deadline {
            bail!(
                "WASAPI prebuffer timed out after {} chunks; play audio on Windows and retry",
                prebuffered
            );
        }

        if let Ok(chunk) = capture.recv_timeout(Duration::from_millis(250)) {
            let frame = LivePcmFrame {
                samples: chunk.samples,
                channels: chunk.channels as u8,
                sample_rate: chunk.sample_rate,
            };
            if sender.try_send(frame) {
                prebuffered += 1;
            }
        }
    }

    client.start_live_streaming_with_decoder(decoder).await?;

    let started = Instant::now();
    let mut sent = 0u64;
    let mut dropped = 0u64;

    while started.elapsed() < stream_duration {
        if let Ok(chunk) = capture.recv_timeout(Duration::from_millis(250)) {
            let frame = LivePcmFrame {
                samples: chunk.samples,
                channels: chunk.channels as u8,
                sample_rate: chunk.sample_rate,
            };

            if sender.try_send(frame) {
                sent += 1;
            } else {
                dropped += 1;
            }
        }
    }

    capture.stop();
    drop(sender);

    tokio::time::timeout(Duration::from_secs(5), client.disconnect())
        .await
        .map_err(|_| anyhow!("AirPlay TEARDOWN timed out after streaming"))??;

    Ok(StreamTestResult {
        session,
        prebuffer_chunks: prebuffered,
        sent_chunks: sent,
        dropped_chunks: dropped,
        stream_duration: started.elapsed(),
    })
}
