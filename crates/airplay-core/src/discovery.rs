use airplay2_discovery::{Discovery, ServiceBrowser};
use anyhow::Result;
use crate::device_profile::DeviceKind;
use std::net::IpAddr;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct AirPlayReceiver {
    pub id: String,
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
            let id = d.id.to_mac_string();
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
                id,
                name: repair_mojibake(&d.name),
                model: repair_mojibake(&d.model),
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


impl AirPlayReceiver {
    pub fn device_kind(&self) -> DeviceKind {
        DeviceKind::from_model(&self.model)
    }

    pub fn friendly_model_name(&self) -> &'static str {
        self.device_kind().friendly_name()
    }

    pub fn preferred_address(&self) -> Option<&str> {
        self.addresses
            .iter()
            .find(|value| value.parse::<IpAddr>().map(|ip| ip.is_ipv4()).unwrap_or(false))
            .or_else(|| self.addresses.first())
            .map(String::as_str)
    }
}


fn repair_mojibake(input: &str) -> String {
    // mDNS names should be UTF-8, but some Windows/network stacks surface UTF-8
    // bytes as Latin-1 text (e.g. "Phòng" -> "PhÃ²ng"). Only attempt recovery
    // when the string contains classic mojibake markers.
    let suspicious = input.contains('Ã')
        || input.contains('Â')
        || input.contains('Æ')
        || input.contains('á')
        || input.contains('Ä');

    if !suspicious {
        return input.to_string();
    }

    let mut bytes = Vec::with_capacity(input.len());
    for ch in input.chars() {
        let code = ch as u32;
        if code > 0xFF {
            return input.to_string();
        }
        bytes.push(code as u8);
    }

    match String::from_utf8(bytes) {
        Ok(decoded) => decoded,
        Err(_) => input.to_string(),
    }
}

#[cfg(test)]
mod unicode_tests {
    use super::repair_mojibake;

    #[test]
    fn repairs_common_vietnamese_utf8_mojibake() {
        assert_eq!(repair_mojibake("PhÃ²ng khÃ¡ch"), "Phòng khách");
        assert_eq!(repair_mojibake("Loa bÃªn pháº£i"), "Loa bên phải");
    }

    #[test]
    fn leaves_valid_unicode_unchanged() {
        assert_eq!(repair_mojibake("Phòng khách"), "Phòng khách");
    }
}
