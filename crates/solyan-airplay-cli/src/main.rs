use anyhow::Result;
use audio_capture::{start_default_loopback, AudioFormat};
use solyan_airplay_core::discovery::discover_once;
use solyan_airplay_core::session::{connect_test, stream_test};
use std::time::{Duration, Instant};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    println!("SolYan AirPlay2 v0.1.4");

    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("--capture-test") => capture_test(),
        Some("--connect-test") => {
            let selector = args.get(2).map(String::as_str);
            session_test(selector).await
        }
        Some("--stream-test") => {
            let selector = args.get(2).map(String::as_str);
            live_stream_test(selector).await
        }
        Some("--help") | Some("-h") => {
            print_help();
            Ok(())
        }
        Some(other) => {
            eprintln!("Unknown option: {other}");
            print_help();
            Ok(())
        }
        None => scan().await,
    }
}

async fn scan() -> Result<()> {
    println!("Scanning for AirPlay/AirPlay 2 receivers for 4 seconds...");
    let devices = discover_once(Duration::from_secs(4)).await?;

    if devices.is_empty() {
        println!("No AirPlay receivers found.");
        return Ok(());
    }

    for d in devices {
        println!();
        println!("Device: {}", d.name);
        println!("  model: {}", d.model);
        println!("  address: {:?}:{}", d.addresses, d.port);
        println!("  srcvers: {}", d.source_version);
        println!("  features: 0x{:016X}", d.features_raw);
        println!("  audio: {}", yes_no(d.supports_audio));
        println!("  AirPlay 2 buffered: {}", yes_no(d.supports_airplay2));
        println!("  PTP: {}", yes_no(d.supports_ptp));
        println!("  buffered feature: {}", yes_no(d.supports_buffered_audio));
        println!(
            "  transient pairing: {}",
            yes_no(d.supports_transient_pairing)
        );
        println!("  password required: {}", yes_no(d.requires_password));
    }

    Ok(())
}

async fn session_test(selector: Option<&str>) -> Result<()> {
    println!("Testing real AirPlay 2 session handshake...");
    if let Some(selector) = selector {
        println!("Receiver selector: {selector}");
    }

    let result = connect_test(
        selector,
        Duration::from_secs(4),
        Duration::from_secs(20),
    )
    .await?;

    println!("Connected and cleanly disconnected:");
    println!("  device: {}", result.name);
    println!("  model: {}", result.model);
    println!("  address: {}", result.address);
    println!(
        "  AirPlay 2 buffered advertised: {}",
        yes_no(result.supports_airplay2)
    );
    println!("  PTP advertised: {}", yes_no(result.supports_ptp));
    println!("RESULT: pairing + encrypted RTSP SETUP + TEARDOWN succeeded.");
    Ok(())
}

async fn live_stream_test(selector: Option<&str>) -> Result<()> {
    println!("Testing live Windows audio -> HomePod over AirPlay...");
    println!("Play audio on the PC now. The stream test runs for 10 seconds.");
    if let Some(selector) = selector {
        println!("Receiver selector: {selector}");
    }

    let result = stream_test(
        selector,
        Duration::from_secs(4),
        Duration::from_secs(20),
        Duration::from_secs(10),
    )
    .await?;

    println!("Stream completed:");
    println!("  device: {}", result.session.name);
    println!("  model: {}", result.session.model);
    println!("  address: {}", result.session.address);
    println!("  prebuffer chunks: {}", result.prebuffer_chunks);
    println!("  live chunks sent: {}", result.sent_chunks);
    println!("  live chunks dropped: {}", result.dropped_chunks);
    println!(
        "  measured test duration: {:.2}s",
        result.stream_duration.as_secs_f64()
    );

    if result.sent_chunks == 0 {
        println!("RESULT: session connected, but no live PCM chunks were sent.");
    } else if result.dropped_chunks == 0 {
        println!("RESULT: live PCM -> ALAC -> encrypted AirPlay RTP completed with no queue drops.");
    } else {
        println!("RESULT: live AirPlay stream completed with queue drops; inspect timing/buffer settings.");
    }

    Ok(())
}

fn capture_test() -> Result<()> {
    println!("Starting Windows WASAPI loopback test...");
    let mut capture = start_default_loopback(AudioFormat::default())?;
    println!(
        "Capturing: {} @ {} Hz / {} ch / {} bit",
        capture.device_name,
        capture.format.sample_rate,
        capture.format.channels,
        capture.format.bits_per_sample
    );

    let start = Instant::now();
    let mut chunks = 0u64;
    let mut peak = 0i16;

    while start.elapsed() < Duration::from_secs(5) {
        if let Ok(chunk) = capture.recv_timeout(Duration::from_millis(500)) {
            chunks += 1;
            if let Some(p) = chunk.samples.iter().map(|s| s.saturating_abs()).max() {
                peak = peak.max(p);
            }
        }
    }

    capture.stop();
    println!("Captured chunks: {chunks}");
    println!("Peak sample: {peak}");
    if chunks == 0 {
        println!("RESULT: no PCM captured; play audio in Windows and retry.");
    } else {
        println!("RESULT: WASAPI loopback is working.");
    }

    Ok(())
}

fn print_help() {
    println!("Usage:");
    println!("  solyan-airplay.exe");
    println!("      Scan AirPlay receivers and report capabilities.");
    println!("  solyan-airplay.exe --capture-test");
    println!("      Capture Windows system audio for five seconds.");
    println!("  solyan-airplay.exe --connect-test [name-or-ip]");
    println!("      Perform a real AirPlay pairing/session SETUP and TEARDOWN.");
    println!("  solyan-airplay.exe --stream-test [name-or-ip]");
    println!("      Capture Windows audio and stream it to HomePod for ten seconds.");
}

fn yes_no(v: bool) -> &'static str {
    if v { "YES" } else { "NO" }
}
