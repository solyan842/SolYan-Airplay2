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
                name: decode_dns_sd_name(&d.name),
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


fn decode_dns_sd_name(input: &str) -> String {
    // DNS-SD instance names may contain escaped UTF-8 bytes such as
    // "\195\178" as well as escaped dots/backslashes. Decode bytes first,
    // then reconstruct Unicode so Vietnamese device names display correctly.
    let bytes = input.as_bytes();
    let mut out = Vec::<u8>::with_capacity(bytes.len());
    let mut i = 0usize;

    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            if i + 3 < bytes.len()
                && bytes[i + 1].is_ascii_digit()
                && bytes[i + 2].is_ascii_digit()
                && bytes[i + 3].is_ascii_digit()
            {
                let code = ((bytes[i + 1] - b'0') as u16) * 100
                    + ((bytes[i + 2] - b'0') as u16) * 10
                    + (bytes[i + 3] - b'0') as u16;
                if code <= 255 {
                    out.push(code as u8);
                    i += 4;
                    continue;
                }
            }

            // DNS-SD also escapes punctuation as "\." and "\\".
            out.push(bytes[i + 1]);
            i += 2;
            continue;
        }

        out.push(bytes[i]);
        i += 1;
    }

    String::from_utf8(out).unwrap_or_else(|e| {
        String::from_utf8_lossy(e.as_bytes()).into_owned()
    })
}

#[cfg(test)]
mod unicode_name_tests {
    use super::decode_dns_sd_name;

    #[test]
    fn preserves_native_utf8_vietnamese() {
        assert_eq!(decode_dns_sd_name("Phòng Khách"), "Phòng Khách");
    }

    #[test]
    fn decodes_dns_sd_escaped_utf8_bytes() {
        assert_eq!(
            decode_dns_sd_name(r"Ph\195\178ng Kh\195\161ch"),
            "Phòng Khách"
        );
    }

    #[test]
    fn decodes_escaped_punctuation() {
        assert_eq!(decode_dns_sd_name(r"Phòng\.Khách"), "Phòng.Khách");
    }
}
