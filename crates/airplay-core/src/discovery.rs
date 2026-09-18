use anyhow::Result;
use mdns_sd::{ServiceDaemon, ServiceEvent};
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct AirPlayReceiver {
    pub fullname: String,
    pub hostname: String,
    pub port: u16,
    pub addresses: Vec<String>,
}

pub fn discover_once(timeout: Duration) -> Result<Vec<AirPlayReceiver>> {
    let mdns = ServiceDaemon::new()?;
    let rx = mdns.browse("_airplay._tcp.local.")?;
    let deadline = std::time::Instant::now() + timeout;
    let mut out = Vec::new();

    while std::time::Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(ServiceEvent::ServiceResolved(info)) => {
                let receiver = AirPlayReceiver {
                    fullname: info.get_fullname().to_string(),
                    hostname: info.get_hostname().to_string(),
                    port: info.get_port(),
                    addresses: info.get_addresses().iter().map(ToString::to_string).collect(),
                };
                if !out.iter().any(|x: &AirPlayReceiver| x.fullname == receiver.fullname) {
                    out.push(receiver);
                }
            }
            Ok(_) => {}
            Err(_) => {}
        }
    }

    let _ = mdns.stop_browse("_airplay._tcp.local.");
    let _ = mdns.shutdown();
    Ok(out)
}
