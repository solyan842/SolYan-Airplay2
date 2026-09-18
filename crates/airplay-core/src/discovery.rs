use airplay2_discovery::{Discovery, ServiceBrowser};
use anyhow::Result;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct AirPlayReceiver {
    pub name: String,
    pub model: String,
    pub port: u16,
    pub addresses: Vec<String>,
    pub source_version: String,
    pub features_raw: u64,
    pub supports_audio: bool,
    pub supports_airplay2: bool,
    pub supports_ptp: bool,
    pub supports_buffered_audio: bool,
    pub supports_transient_pairing: bool,
    pub requires_password: bool,
}

pub async fn discover_once(timeout: Duration) -> Result<Vec<AirPlayReceiver>> {
    let browser = ServiceBrowser::new()?;
    let devices = browser.scan(timeout).await?;

    Ok(devices
        .into_iter()
        .map(|d| {
            let supports_airplay2 = d.supports_airplay2();
            let supports_ptp = d.supports_ptp();
            let supports_audio = d.features.supports_audio();
            let supports_buffered_audio = d.features.supports_buffered_audio();
            let supports_transient_pairing = d.features.supports_transient_pairing();
            let features_raw = d.features.raw();
            let source_version = format!(
                "{}.{}.{}",
                d.source_version.major, d.source_version.minor, d.source_version.patch
            );
            let addresses = d.addresses.iter().map(ToString::to_string).collect();

            AirPlayReceiver {
                name: d.name,
                model: d.model,
                port: d.port,
                addresses,
                source_version,
                features_raw,
                supports_audio,
                supports_airplay2,
                supports_ptp,
                supports_buffered_audio,
                supports_transient_pairing,
                requires_password: d.requires_password,
            }
        })
        .collect())
}
