use airplay2_client::AirPlayClient;
use anyhow::{anyhow, bail, Result};
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct SessionTestResult {
    pub name: String,
    pub model: String,
    pub address: String,
    pub supports_airplay2: bool,
    pub supports_ptp: bool,
}

pub async fn connect_test(
    selector: Option<&str>,
    discovery_timeout: Duration,
    connect_timeout: Duration,
) -> Result<SessionTestResult> {
    let mut client = AirPlayClient::new()?;
    client.set_render_delay_ms(200);

    let mut devices = client.discover(discovery_timeout).await?;
    devices.retain(|d| d.features.supports_audio());

    if devices.is_empty() {
        bail!("no AirPlay audio receivers found");
    }

    let selected = if let Some(selector) = selector {
        let needle = selector.to_lowercase();
        devices
            .into_iter()
            .find(|d| {
                d.name.to_lowercase().contains(&needle)
                    || d.model.to_lowercase().contains(&needle)
                    || d.addresses.iter().any(|ip| ip.to_string() == selector)
            })
            .ok_or_else(|| anyhow!("no AirPlay audio receiver matched '{selector}'"))?
    } else if devices.len() == 1 {
        devices.remove(0)
    } else {
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
            homepod_matches.into_iter().next().unwrap()
        } else {
            let choices = devices
                .iter()
                .map(|d| format!("{} ({})", d.name, d.model))
                .collect::<Vec<_>>()
                .join(", ");
            bail!("multiple AirPlay audio receivers found; rerun with --connect-test <name-or-ip>. Choices: {choices}");
        }
    };

    let address = selected
        .addresses
        .iter()
        .find(|ip| ip.is_ipv4())
        .or_else(|| selected.addresses.first())
        .map(ToString::to_string)
        .unwrap_or_else(|| "<unknown>".to_string());

    let result = SessionTestResult {
        name: selected.name.clone(),
        model: selected.model.clone(),
        address,
        supports_airplay2: selected.supports_airplay2(),
        supports_ptp: selected.supports_ptp(),
    };

    tokio::time::timeout(connect_timeout, client.connect(&selected))
        .await
        .map_err(|_| anyhow!("AirPlay connect/setup timed out after {connect_timeout:?}"))??;

    tokio::time::timeout(Duration::from_secs(5), client.disconnect())
        .await
        .map_err(|_| anyhow!("AirPlay TEARDOWN timed out"))??;

    Ok(result)
}
