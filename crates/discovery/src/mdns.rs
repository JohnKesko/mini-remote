//! Listening for computers that announce themselves over Bonjour / mDNS.

use std::net::IpAddr;

use mdns_sd::{ResolvedService, ServiceDaemon, ServiceEvent};
use tokio::sync::mpsc::UnboundedSender;
use tokio::task::JoinSet;

use crate::identify::{Mac, clean_hostname, parse_mac};

const RFB: &str = "_rfb._tcp.local.";
const SSH: &str = "_ssh._tcp.local.";
const SFTP: &str = "_sftp-ssh._tcp.local.";
const WORKSTATION: &str = "_workstation._tcp.local.";
const DEVICE_INFO: &str = "_device-info._tcp.local.";

/// The announcements we listen for.
const SERVICE_TYPES: [&str; 5] = [RFB, SSH, SFTP, WORKSTATION, DEVICE_INFO];

/// What kind of service was announced.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ServiceKind {
    /// Screen sharing (VNC).
    Rfb,
    /// Secure shell or secure file transfer.
    Ssh,
    /// "I am a computer" (sent by Linux computers running Avahi, such as a Pi).
    Workstation,
    /// Model information.
    DeviceInfo,
}

/// One announcement, reduced to what we need.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Announcement {
    pub kind: ServiceKind,
    /// The instance name, for example "Living room Pi".
    pub instance: String,
    /// For example `raspberrypi.local`.
    pub hostname: Option<String>,
    pub port: u16,
    /// IPv4 addresses when there are any, otherwise usable IPv6 ones.
    pub addresses: Vec<IpAddr>,
    /// Hardware address, when the name carries one.
    pub mac: Option<Mac>,
    /// Model text from the device information, if any.
    pub model: Option<String>,
}

/// Keeps the mDNS background thread alive; stops it when dropped.
pub(crate) struct MdnsBrowser {
    daemon: ServiceDaemon,
    _forwarders: JoinSet<()>,
}

impl Drop for MdnsBrowser {
    fn drop(&mut self) {
        let _ = self.daemon.shutdown();
    }
}

/// Starts listening. Announcements are sent to `sink`. Returns `None` (after
/// logging) if mDNS cannot be used on this computer.
pub(crate) fn start(sink: UnboundedSender<Announcement>) -> Option<MdnsBrowser> {
    let daemon = match ServiceDaemon::new() {
        Ok(daemon) => daemon,
        Err(err) => {
            log::warn!("mDNS is not available: {err}");
            return None;
        }
    };
    let mut forwarders = JoinSet::new();
    let mut browsing = 0;
    for service_type in SERVICE_TYPES {
        match daemon.browse(service_type) {
            Ok(events) => {
                browsing += 1;
                let sink = sink.clone();
                forwarders.spawn(async move {
                    while let Ok(event) = events.recv_async().await {
                        let ServiceEvent::ServiceResolved(service) = event else {
                            continue;
                        };
                        let Some(announcement) = announcement_from(&service) else {
                            continue;
                        };
                        if sink.send(announcement).is_err() {
                            return;
                        }
                    }
                });
            }
            Err(err) => log::warn!("could not browse {service_type}: {err}"),
        }
    }
    if browsing == 0 {
        let _ = daemon.shutdown();
        return None;
    }
    Some(MdnsBrowser {
        daemon,
        _forwarders: forwarders,
    })
}

/// Reduces a resolved service to an [`Announcement`].
fn announcement_from(service: &ResolvedService) -> Option<Announcement> {
    let kind = kind_of(&service.ty_domain)?;
    let instance = instance_label(&service.fullname, &service.ty_domain);
    let (instance, mac) = if kind == ServiceKind::Workstation {
        split_workstation_name(&instance)
    } else {
        (instance, None)
    };
    let addresses = pick_addresses(service.addresses.iter().map(|a| a.to_ip_addr()));
    if addresses.is_empty() {
        return None;
    }
    let model = service
        .txt_properties
        .get_property_val_str("model")
        .filter(|model| !model.is_empty())
        .map(str::to_string);
    Some(Announcement {
        kind,
        instance,
        hostname: clean_hostname(&service.host),
        port: service.port,
        addresses,
        mac,
        model,
    })
}

/// Maps a service type such as `_rfb._tcp.local.` to its kind.
pub(crate) fn kind_of(service_type: &str) -> Option<ServiceKind> {
    match service_type.to_ascii_lowercase().as_str() {
        RFB => Some(ServiceKind::Rfb),
        SSH | SFTP => Some(ServiceKind::Ssh),
        WORKSTATION => Some(ServiceKind::Workstation),
        DEVICE_INFO => Some(ServiceKind::DeviceInfo),
        _ => None,
    }
}

/// Cuts the service type off a full name:
/// `My Pi._rfb._tcp.local.` becomes `My Pi`.
pub(crate) fn instance_label(full_name: &str, service_type: &str) -> String {
    full_name
        .strip_suffix(service_type)
        .map(|name| name.strip_suffix('.').unwrap_or(name))
        .unwrap_or(full_name)
        .to_string()
}

/// Avahi names its `_workstation` entry `name [hardware address]`. Splits the
/// two parts.
pub(crate) fn split_workstation_name(instance: &str) -> (String, Option<Mac>) {
    let parsed = instance
        .trim_end()
        .strip_suffix(']')
        .and_then(|without_bracket| without_bracket.rsplit_once('['))
        .and_then(|(name, mac_text)| Some((name, parse_mac(mac_text)?)));
    match parsed {
        Some((name, mac)) => (name.trim_end().to_string(), Some(mac)),
        None => (instance.to_string(), None),
    }
}

/// Prefers IPv4 addresses. IPv6 addresses are only used when there is no
/// IPv4 address, and never the link-local ones (they cannot be used alone).
pub(crate) fn pick_addresses(addresses: impl Iterator<Item = IpAddr>) -> Vec<IpAddr> {
    let usable: Vec<IpAddr> = addresses
        .filter(|address| !address.is_loopback() && !address.is_unspecified())
        .collect();
    let mut chosen: Vec<IpAddr> = usable.iter().copied().filter(IpAddr::is_ipv4).collect();
    if chosen.is_empty() {
        chosen = usable
            .into_iter()
            .filter(|address| match address {
                IpAddr::V6(v6) => !v6.is_unicast_link_local(),
                IpAddr::V4(_) => false,
            })
            .collect();
    }
    chosen.sort();
    chosen.dedup();
    chosen
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, Ipv6Addr};

    use super::*;

    #[test]
    fn maps_service_types() {
        assert_eq!(kind_of("_rfb._tcp.local."), Some(ServiceKind::Rfb));
        assert_eq!(kind_of("_ssh._tcp.local."), Some(ServiceKind::Ssh));
        assert_eq!(kind_of("_sftp-ssh._tcp.local."), Some(ServiceKind::Ssh));
        assert_eq!(
            kind_of("_Workstation._tcp.local."),
            Some(ServiceKind::Workstation)
        );
        assert_eq!(
            kind_of("_device-info._tcp.local."),
            Some(ServiceKind::DeviceInfo)
        );
        assert_eq!(kind_of("_http._tcp.local."), None);
    }

    #[test]
    fn cuts_the_service_type_off() {
        assert_eq!(instance_label("My Pi._rfb._tcp.local.", RFB), "My Pi");
        assert_eq!(instance_label("a.b._rfb._tcp.local.", RFB), "a.b");
        assert_eq!(instance_label("unrelated", RFB), "unrelated");
    }

    #[test]
    fn reads_the_hardware_address_from_a_workstation_name() {
        let (name, mac) = split_workstation_name("kitchen [b8:27:eb:01:02:03]");
        assert_eq!(name, "kitchen");
        assert_eq!(mac, Some([0xB8, 0x27, 0xEB, 1, 2, 3]));

        let (name, mac) = split_workstation_name("plain name");
        assert_eq!(name, "plain name");
        assert_eq!(mac, None);

        let (name, mac) = split_workstation_name("odd [not a mac]");
        assert_eq!(name, "odd [not a mac]");
        assert_eq!(mac, None);
    }

    #[test]
    fn prefers_ipv4_and_skips_useless_addresses() {
        let v4 = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 9));
        let v6_global = IpAddr::V6("2001:db8::9".parse::<Ipv6Addr>().unwrap());
        let v6_link = IpAddr::V6("fe80::9".parse::<Ipv6Addr>().unwrap());
        let loopback = IpAddr::V4(Ipv4Addr::LOCALHOST);

        assert_eq!(
            pick_addresses([v6_global, v4, v6_link, loopback].into_iter()),
            vec![v4]
        );
        assert_eq!(
            pick_addresses([v6_link, v6_global].into_iter()),
            vec![v6_global]
        );
        assert!(pick_addresses([v6_link, loopback].into_iter()).is_empty());
    }
}
