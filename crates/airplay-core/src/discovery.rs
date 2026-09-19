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
    pub group_id: Option<String>,
    pub is_group_leader: bool,
    pub group_public_name: Option<String>,
    pub group_contains_discoverable_leader: bool,
    pub parent_group_id: Option<String>,
    pub tight_sync_id: Option<String>,
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
                group_id: d.group_id.map(|v| v.to_string()),
                is_group_leader: d.is_group_leader,
                group_public_name: d.group_public_name.map(|v| decode_dns_sd_name(&v)),
                group_contains_discoverable_leader: d.group_contains_discoverable_leader,
                parent_group_id: d.parent_group_id.map(|v| v.to_string()),
                tight_sync_id: d.tight_sync_id.map(|v| v.to_string()),
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


#[derive(Debug, Clone)]
pub struct HomePodPair {
    pub id: String,
    pub name: String,
    pub leader_id: String,
    pub member_ids: Vec<String>,
    pub member_names: Vec<String>,
    pub group_id: Option<String>,
    pub tight_sync_id: Option<String>,
}

pub fn detect_homepod_pairs(devices: &[AirPlayReceiver]) -> Vec<HomePodPair> {
    use std::collections::{BTreeMap, BTreeSet};

    let mut by_tsid: BTreeMap<String, Vec<&AirPlayReceiver>> = BTreeMap::new();
    let mut by_gid: BTreeMap<String, Vec<&AirPlayReceiver>> = BTreeMap::new();

    for device in devices.iter().filter(|d| d.device_kind().is_homepod()) {
        if let Some(tsid) = &device.tight_sync_id {
            by_tsid.entry(tsid.clone()).or_default().push(device);
        }
        if let Some(gid) = &device.group_id {
            by_gid.entry(gid.clone()).or_default().push(device);
        }
    }

    let mut pairs = Vec::new();
    let mut consumed = BTreeSet::<String>::new();

    for (tsid, members) in by_tsid {
        if members.len() != 2 {
            continue;
        }
        let key = format!("tsid:{tsid}");
        consumed.extend(members.iter().map(|d| d.id.clone()));
        pairs.push(build_homepod_pair(key, members, Some(tsid), None));
    }

    for (gid, members) in by_gid {
        if members.len() != 2 || members.iter().all(|d| consumed.contains(&d.id)) {
            continue;
        }

        // gid can also describe an ad-hoc multiroom group. Use it as a
        // stereo-pair fallback only when Bonjour exposes leader/group metadata.
        let confident = members.iter().any(|d| d.is_group_leader)
            || members.iter().any(|d| d.group_contains_discoverable_leader)
            || members.iter().any(|d| d.group_public_name.is_some());
        if !confident {
            continue;
        }

        let key = format!("gid:{gid}");
        pairs.push(build_homepod_pair(key, members, None, Some(gid)));
    }

    pairs.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    pairs
}

fn build_homepod_pair(
    id: String,
    mut members: Vec<&AirPlayReceiver>,
    tight_sync_id: Option<String>,
    group_id: Option<String>,
) -> HomePodPair {
    members.sort_by_key(|d| (!d.is_group_leader, d.name.to_lowercase()));

    let leader = members
        .iter()
        .copied()
        .find(|d| d.is_group_leader)
        .unwrap_or(members[0]);

    let name = members
        .iter()
        .find_map(|d| d.group_public_name.clone())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| {
            let names = members.iter().map(|d| d.name.as_str()).collect::<Vec<_>>();
            format!("{} + {}", names[0], names[1])
        });

    HomePodPair {
        id,
        name,
        leader_id: leader.id.clone(),
        member_ids: members.iter().map(|d| d.id.clone()).collect(),
        member_names: members.iter().map(|d| d.name.clone()).collect(),
        group_id: group_id.or_else(|| leader.group_id.clone()),
        tight_sync_id: tight_sync_id.or_else(|| leader.tight_sync_id.clone()),
    }
}

#[cfg(test)]
mod homepod_pair_tests {
    use super::*;

    fn hp(id: &str, name: &str, tsid: Option<&str>, gid: Option<&str>, leader: bool) -> AirPlayReceiver {
        AirPlayReceiver {
            id: id.into(),
            name: name.into(),
            model: "AudioAccessory5,1".into(),
            port: 7000,
            addresses: vec!["192.168.1.10".into()],
            source_version: "400.0.0".into(),
            features_raw: 0,
            supports_audio: true,
            supports_airplay2: true,
            supports_ptp: true,
            supports_buffered_audio: true,
            supports_transient_pairing: true,
            requires_password: false,
            group_id: gid.map(str::to_string),
            is_group_leader: leader,
            group_public_name: Some("Phòng ngủ".into()),
            group_contains_discoverable_leader: leader,
            parent_group_id: None,
            tight_sync_id: tsid.map(str::to_string),
        }
    }

    #[test]
    fn detects_tight_sync_pair() {
        let devices = vec![
            hp("A", "Left", Some("PAIR-1"), Some("G1"), true),
            hp("B", "Right", Some("PAIR-1"), Some("G1"), false),
        ];
        let pairs = detect_homepod_pairs(&devices);
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].name, "Phòng ngủ");
        assert_eq!(pairs[0].leader_id, "A");
        assert_eq!(pairs[0].member_ids.len(), 2);
    }
}
