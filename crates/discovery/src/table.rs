//! The list of computers found so far, and the rules for announcing them.

use std::collections::HashMap;
use std::net::IpAddr;

use tokio::sync::mpsc::UnboundedSender;

use crate::identify::{clean_hostname, hostname_hints_raspberry_pi, is_local_name};
use crate::{DiscoveredHost, DiscoveryEvent};

impl DiscoveredHost {
    /// A computer about which nothing is known yet.
    pub fn new(address: IpAddr) -> Self {
        Self {
            address,
            hostname: None,
            vnc_ports: Vec::new(),
            rfb_version: None,
            ssh: false,
            is_raspberry_pi: false,
            service_name: None,
        }
    }

    /// Adds a port that answered like VNC. The version of the lowest port wins.
    pub(crate) fn add_vnc_port(&mut self, port: u16, version: &str) {
        if let Err(position) = self.vnc_ports.binary_search(&port) {
            self.vnc_ports.insert(position, port);
        }
        if self.vnc_ports.first() == Some(&port) {
            self.rfb_version = Some(version.to_string());
        }
    }

    /// Remembers a host name. Names ending in `.local` are preferred because
    /// they work for connecting. A Pi-like name also marks it as a Pi.
    pub(crate) fn set_hostname(&mut self, name: &str) {
        let Some(name) = clean_hostname(name) else {
            return;
        };
        if hostname_hints_raspberry_pi(&name) {
            self.is_raspberry_pi = true;
        }
        let better = match &self.hostname {
            None => true,
            Some(old) => !is_local_name(old) && is_local_name(&name),
        };
        if better {
            self.hostname = Some(name);
        }
    }

    /// Takes over what was learned about the same computer at another address.
    pub(crate) fn absorb(&mut self, other: &DiscoveredHost) {
        for &port in &other.vnc_ports {
            if let Err(position) = self.vnc_ports.binary_search(&port) {
                self.vnc_ports.insert(position, port);
            }
        }
        if self.rfb_version.is_none() {
            self.rfb_version = other.rfb_version.clone();
        }
        self.ssh |= other.ssh;
        self.is_raspberry_pi |= other.is_raspberry_pi;
        if self.service_name.is_none() {
            self.service_name = other.service_name.clone();
        }
        if let Some(name) = &other.hostname {
            self.set_hostname(name);
        }
    }

    /// Worth showing: offers screen sharing or secure shell, or is a Pi.
    fn is_interesting(&self) -> bool {
        !self.vnc_ports.is_empty() || self.ssh || self.is_raspberry_pi
    }
}

/// Keeps every computer by address and sends `Found` / `Updated` / `Removed`
/// events. One computer seen at several addresses with the same name (a Pi on
/// both Wi-Fi and cable, or with an IPv4 and an IPv6 address) is listed once,
/// under its best address, with what was learned at all of them.
pub(crate) struct HostTable {
    /// What each address told us itself.
    hosts: HashMap<IpAddr, DiscoveredHost>,
    /// What was last sent for each listed address.
    shown: HashMap<IpAddr, DiscoveredHost>,
    events: UnboundedSender<DiscoveryEvent>,
}

impl HostTable {
    pub(crate) fn new(events: UnboundedSender<DiscoveryEvent>) -> Self {
        Self {
            hosts: HashMap::new(),
            shown: HashMap::new(),
            events,
        }
    }

    /// Changes what we know about an address. If the computer is worth showing
    /// and something changed, the matching events are sent.
    pub(crate) fn update(&mut self, address: IpAddr, change: impl FnOnce(&mut DiscoveredHost)) {
        let host = self
            .hosts
            .entry(address)
            .or_insert_with(|| DiscoveredHost::new(address));
        let before = host.clone();
        change(host);
        if *host != before {
            self.refresh(address);
        }
    }

    /// Recomputes the listing for the computer that `address` belongs to.
    fn refresh(&mut self, address: IpAddr) {
        let name = self.hosts.get(&address).and_then(|h| h.hostname.clone());
        let group: Vec<IpAddr> = match &name {
            Some(name) => self
                .hosts
                .iter()
                .filter(|(_, h)| h.hostname.as_deref().is_some_and(|n| n.eq_ignore_ascii_case(name)))
                .map(|(a, _)| *a)
                .collect(),
            None => vec![address],
        };
        // Best address first: screen sharing, then IPv4, then SSH, then the
        // one already shown (so the list does not jump around).
        let rank = |a: &IpAddr| {
            let h = &self.hosts[a];
            (
                !h.vnc_ports.is_empty(),
                a.is_ipv4(),
                h.ssh,
                self.shown.contains_key(a),
                std::cmp::Reverse(*a),
            )
        };
        let best = *group.iter().max_by_key(|a| rank(a)).expect("group holds address");

        let mut combined = self.hosts[&best].clone();
        for other in group.iter().filter(|a| **a != best) {
            if self.shown.remove(other).is_some() {
                // Nobody listening any more is not an error.
                let _ = self.events.send(DiscoveryEvent::Removed(*other));
            }
            combined.absorb(&self.hosts[other]);
        }
        if !combined.is_interesting() {
            return;
        }
        let event = match self.shown.get(&best) {
            Some(previous) if *previous == combined => return,
            Some(_) => DiscoveryEvent::Updated(combined.clone()),
            None => DiscoveryEvent::Found(combined.clone()),
        };
        self.shown.insert(best, combined);
        let _ = self.events.send(event);
    }

    /// What an address told us (even if it was not announced yet).
    pub(crate) fn get(&self, address: IpAddr) -> Option<&DiscoveredHost> {
        self.hosts.get(&address)
    }

    /// True once the receiving side has been dropped.
    pub(crate) fn is_closed(&self) -> bool {
        self.events.is_closed()
    }
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

    use super::*;

    const HOST: IpAddr = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 10));
    const HOST_V6: IpAddr = IpAddr::V6(std::net::Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0, 0x10));

    fn drain(rx: &mut UnboundedReceiver<DiscoveryEvent>) -> Vec<DiscoveryEvent> {
        std::iter::from_fn(|| rx.try_recv().ok()).collect()
    }

    fn table() -> (HostTable, UnboundedReceiver<DiscoveryEvent>) {
        let (tx, rx) = unbounded_channel();
        (HostTable::new(tx), rx)
    }

    #[test]
    fn found_first_then_updated() {
        let (mut table, mut rx) = table();
        table.update(HOST, |h| h.add_vnc_port(5900, "3.8"));
        table.update(HOST, |h| h.set_hostname("box.local."));
        table.update(HOST, |h| h.set_hostname("box.local"));
        let Some(DiscoveryEvent::Found(found)) = rx.try_recv().ok() else {
            panic!("expected Found");
        };
        assert_eq!(found.vnc_ports, vec![5900]);
        assert_eq!(found.hostname, None);
        let Some(DiscoveryEvent::Updated(updated)) = rx.try_recv().ok() else {
            panic!("expected Updated");
        };
        assert_eq!(updated.hostname.as_deref(), Some("box.local"));
        // The second identical change sends nothing.
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn boring_computers_stay_quiet_until_they_matter() {
        let (mut table, mut rx) = table();
        table.update(HOST, |h| h.set_hostname("laptop.local"));
        assert!(rx.try_recv().is_err());
        table.update(HOST, |h| h.ssh = true);
        let Some(DiscoveryEvent::Found(found)) = rx.try_recv().ok() else {
            panic!("expected Found");
        };
        // What we learned earlier is included.
        assert_eq!(found.hostname.as_deref(), Some("laptop.local"));
        assert!(found.ssh);
    }

    #[test]
    fn pi_like_name_marks_a_pi() {
        let (mut table, mut rx) = table();
        table.update(HOST, |h| h.set_hostname("raspberrypi.local"));
        let Some(DiscoveryEvent::Found(found)) = rx.try_recv().ok() else {
            panic!("expected Found");
        };
        assert!(found.is_raspberry_pi);
    }

    #[test]
    fn ports_stay_sorted_and_lowest_port_gives_the_version() {
        let mut host = DiscoveredHost::new(HOST);
        host.add_vnc_port(5901, "3.889");
        host.add_vnc_port(5900, "3.8");
        host.add_vnc_port(5901, "3.889");
        assert_eq!(host.vnc_ports, vec![5900, 5901]);
        assert_eq!(host.rfb_version.as_deref(), Some("3.8"));
    }

    #[test]
    fn local_names_replace_other_names() {
        let mut host = DiscoveredHost::new(HOST);
        host.set_hostname("box.example.lan");
        host.set_hostname("box.local");
        assert_eq!(host.hostname.as_deref(), Some("box.local"));
        host.set_hostname("other.example.lan");
        assert_eq!(host.hostname.as_deref(), Some("box.local"));
        host.set_hostname("192.0.2.10");
        assert_eq!(host.hostname.as_deref(), Some("box.local"));
    }

    #[test]
    fn closed_receiver_is_noticed() {
        let (table, rx) = table();
        assert!(!table.is_closed());
        drop(rx);
        assert!(table.is_closed());
    }

    #[test]
    fn ipv6_twin_found_first_is_taken_back() {
        let (mut table, mut rx) = table();
        // Seen at its IPv6 address first, with only a name (a Pi-like one).
        table.update(HOST_V6, |h| h.set_hostname("pi-test.local"));
        // Then at its IPv4 address, where screen sharing answers.
        table.update(HOST, |h| h.add_vnc_port(5900, "3.8"));
        table.update(HOST, |h| h.set_hostname("pi-test.local"));
        let events = drain(&mut rx);
        assert!(matches!(&events[0], DiscoveryEvent::Found(h) if h.address == HOST_V6));
        assert!(events.contains(&DiscoveryEvent::Removed(HOST_V6)));
        let DiscoveryEvent::Updated(last) = events.last().unwrap() else { panic!("expected Updated") };
        assert_eq!(last.address, HOST);
        assert_eq!(last.vnc_ports, vec![5900]);
        assert!(last.is_raspberry_pi);
    }

    #[test]
    fn ipv6_twin_found_later_is_never_shown() {
        let (mut table, mut rx) = table();
        table.update(HOST, |h| h.add_vnc_port(5900, "3.8"));
        table.update(HOST, |h| h.set_hostname("Pi-Test.local"));
        drain(&mut rx);
        table.update(HOST_V6, |h| h.set_hostname("pi-test.local"));
        // News about the IPv6 address lands on the IPv4 entry.
        table.update(HOST_V6, |h| h.ssh = true);
        let events = drain(&mut rx);
        assert!(events.iter().all(|e| !matches!(e, DiscoveryEvent::Found(h) if h.address == HOST_V6)));
        let DiscoveryEvent::Updated(last) = events.last().unwrap() else { panic!("expected Updated") };
        assert_eq!(last.address, HOST);
        assert!(last.ssh);
    }

    #[test]
    fn different_names_stay_apart() {
        let (mut table, mut rx) = table();
        table.update(HOST, |h| h.set_hostname("pi-one.local"));
        table.update(HOST_V6, |h| h.set_hostname("pi-two.local"));
        let found: Vec<IpAddr> = drain(&mut rx)
            .into_iter()
            .filter_map(|e| if let DiscoveryEvent::Found(h) = e { Some(h.address) } else { None })
            .collect();
        assert_eq!(found, vec![HOST, HOST_V6]);
    }

    #[test]
    fn same_name_on_two_ipv4_addresses_keeps_the_one_with_screen_sharing() {
        let (mut table, mut rx) = table();
        let cable = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 5));
        let wifi = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 60));
        table.update(cable, |h| h.set_hostname("pi-test.local"));
        table.update(wifi, |h| h.set_hostname("pi-test.local"));
        // Only one entry so far, even though both looked like a Pi.
        let found: Vec<IpAddr> = drain(&mut rx)
            .into_iter()
            .filter_map(|e| if let DiscoveryEvent::Found(h) = e { Some(h.address) } else { None })
            .collect();
        assert_eq!(found, vec![cable]);
        // Screen sharing answers on the Wi-Fi address: that one takes over.
        table.update(wifi, |h| h.add_vnc_port(5900, "3.8"));
        let events = drain(&mut rx);
        assert!(events.contains(&DiscoveryEvent::Removed(cable)), "{events:?}");
        let DiscoveryEvent::Found(last) = events.last().unwrap() else { panic!("expected Found, got {events:?}") };
        assert_eq!(last.address, wifi);
        assert_eq!(last.vnc_ports, vec![5900]);
    }
}
