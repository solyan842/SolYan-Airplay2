#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceKind {
    HomePodMini,
    HomePod1,
    HomePod2,
    HomePodOther,
    AppleTv2,
    AppleTv3,
    AppleTvHd4,
    AppleTv4K1,
    AppleTv4K2,
    AppleTv4K3,
    AppleTvOther,
    AirPlaySpeaker,
}

impl DeviceKind {
    pub fn from_model(model: &str) -> Self {
        match model.trim() {
            "AudioAccessory5,1" | "AudioAccessorySingle5,1" => Self::HomePodMini,
            "AudioAccessory1,1" | "AudioAccessory1,2" => Self::HomePod1,
            "AudioAccessory6,1" => Self::HomePod2,
            "AppleTV2,1" => Self::AppleTv2,
            "AppleTV3,1" | "AppleTV3,2" => Self::AppleTv3,
            "AppleTV5,3" => Self::AppleTvHd4,
            "AppleTV6,2" => Self::AppleTv4K1,
            "AppleTV11,1" => Self::AppleTv4K2,
            "AppleTV14,1" => Self::AppleTv4K3,
            value if value.starts_with("AudioAccessory") => Self::HomePodOther,
            value if value.starts_with("AppleTV") => Self::AppleTvOther,
            _ => Self::AirPlaySpeaker,
        }
    }

    pub fn friendly_name(self) -> &'static str {
        match self {
            Self::HomePodMini => "HomePod mini",
            Self::HomePod1 => "HomePod (1st generation)",
            Self::HomePod2 => "HomePod (2nd generation)",
            Self::HomePodOther => "HomePod",
            Self::AppleTv2 => "Apple TV (2nd generation)",
            Self::AppleTv3 => "Apple TV (3rd generation)",
            Self::AppleTvHd4 => "Apple TV HD (4th generation)",
            Self::AppleTv4K1 => "Apple TV 4K (1st generation)",
            Self::AppleTv4K2 => "Apple TV 4K (2nd generation)",
            Self::AppleTv4K3 => "Apple TV 4K (3rd generation)",
            Self::AppleTvOther => "Apple TV",
            Self::AirPlaySpeaker => "AirPlay speaker",
        }
    }

    pub fn icon_text(self) -> &'static str {
        match self {
            Self::HomePodMini => "mini",
            Self::HomePod1 => "HP1",
            Self::HomePod2 => "HP2",
            Self::HomePodOther => "HP",
            Self::AppleTv2 => "TV2",
            Self::AppleTv3 => "TV3",
            Self::AppleTvHd4 => "TV4",
            Self::AppleTv4K1 => "4K1",
            Self::AppleTv4K2 => "4K2",
            Self::AppleTv4K3 => "4K3",
            Self::AppleTvOther => "TV",
            Self::AirPlaySpeaker => "AP",
        }
    }

    pub fn is_homepod(self) -> bool {
        matches!(
            self,
            Self::HomePodMini | Self::HomePod1 | Self::HomePod2 | Self::HomePodOther
        )
    }

    pub fn is_apple_tv(self) -> bool {
        matches!(
            self,
            Self::AppleTv2
                | Self::AppleTv3
                | Self::AppleTvHd4
                | Self::AppleTv4K1
                | Self::AppleTv4K2
                | Self::AppleTv4K3
                | Self::AppleTvOther
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_known_apple_receivers() {
        assert_eq!(DeviceKind::from_model("AudioAccessory5,1"), DeviceKind::HomePodMini);
        assert_eq!(DeviceKind::from_model("AudioAccessory6,1"), DeviceKind::HomePod2);
        assert_eq!(DeviceKind::from_model("AppleTV3,2"), DeviceKind::AppleTv3);
        assert_eq!(DeviceKind::from_model("AppleTV5,3"), DeviceKind::AppleTvHd4);
        assert_eq!(DeviceKind::from_model("AppleTV14,1"), DeviceKind::AppleTv4K3);
    }
}
