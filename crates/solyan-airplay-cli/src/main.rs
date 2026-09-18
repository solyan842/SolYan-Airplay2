use anyhow::Result;
use audio_capture::{start_default_loopback, AudioFormat};
use solyan_airplay_core::discovery::discover_once;
use solyan_airplay_core::session::connect_test;
use std::time::{Duration, Instant};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    println!("SolYan AirPlay2 v0.1.1");

    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("--capture-test") => capture_test(),
        Some("--connect-test") => {
            let selector = args.get(2).map(String::as_str);
            session_test(selector).await
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
    println!("  AirPlay 2 buffered advertised: {}", yes_no(result.supports_airplay2));
    println!("  PTP advertised: {}", yes_no(result.supports_ptp));
    println!("RESULT: pairing + encrypted RTSP SETUP + TEARDOWN succeeded.");
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
    println!("      Perform a real AirPlay 2 pairing/session SETUP and TEARDOWN.");
}

fn yes_no(v: bool) -> &'static str {
    if v { "YES" } else { "NO" }
}
