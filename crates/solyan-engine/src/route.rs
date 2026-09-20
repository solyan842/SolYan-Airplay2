#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    Raop,
    AirPlay2Compat,
    AirPlay2Native,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReceiverCapabilities {
    pub supports_airplay2: bool,
    pub supports_pairing: bool,
    pub supports_ptp: bool,
    pub has_stored_credentials: bool,
    pub requires_pin: bool,
    pub legacy_pairing: bool,
}

pub struct RouteResolver;

impl RouteResolver {
    pub fn resolve(caps: ReceiverCapabilities) -> Route {
        if !caps.supports_airplay2 {
            return Route::Raop;
        }

        if caps.has_stored_credentials {
            return Route::AirPlay2Native;
        }

        if caps.supports_pairing && !caps.requires_pin && !caps.legacy_pairing {
            return Route::AirPlay2Native;
        }

        Route::AirPlay2Compat
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn airport_style_receiver_routes_to_raop() {
        assert_eq!(
            RouteResolver::resolve(ReceiverCapabilities::default()),
            Route::Raop
        );
    }

    #[test]
    fn stored_credentials_select_native_ap2() {
        let caps = ReceiverCapabilities {
            supports_airplay2: true,
            has_stored_credentials: true,
            ..Default::default()
        };
        assert_eq!(RouteResolver::resolve(caps), Route::AirPlay2Native);
    }

    #[test]
    fn pairing_capable_receiver_can_select_transient_native() {
        let caps = ReceiverCapabilities {
            supports_airplay2: true,
            supports_pairing: true,
            ..Default::default()
        };
        assert_eq!(RouteResolver::resolve(caps), Route::AirPlay2Native);
    }

    #[test]
    fn airplay2_without_safe_native_pairing_falls_back_to_compat() {
        let caps = ReceiverCapabilities {
            supports_airplay2: true,
            supports_pairing: true,
            requires_pin: true,
            ..Default::default()
        };
        assert_eq!(RouteResolver::resolve(caps), Route::AirPlay2Compat);
    }
}
