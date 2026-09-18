use airplay_core::discovery::discover_once;
use anyhow::Result;
use std::time::Duration;

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    println!("SolYan AirPlay2 v0.1.0");
    println!("Scanning for AirPlay receivers for 4 seconds...");

    let devices = discover_once(Duration::from_secs(4))?;
    if devices.is_empty() {
        println!("No AirPlay receivers found.");
    } else {
        for d in devices {
            println!("- {} @ {}:{} {:?}", d.fullname, d.hostname, d.port, d.addresses);
        }
    }

    Ok(())
}
