//! The list of computers found so far, and the rules for announcing them.

use std::collections::{HashMap, HashSet};
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

    /// Worth showing: offers screen sharing or secure shell, or is a Pi.
    fn is_interesting(&self) -> bool {
        !self.vnc_ports.is_empty() || self.ssh || self.is_raspberry_pi
    }
}

/// Keeps every computer by address and sends `Found` / `Updated` events.
pub(crate) struct HostTable {
    hosts: HashMap<IpAddr, DiscoveredHost>,
    announced: HashSet<IpAddr>,
    events: UnboundedSender<DiscoveryEvent>,
}

impl HostTable {
    pub(crate) fn new(events: UnboundedSender<DiscoveryEvent>) -> Self {
        Self {
            hosts: HashMap::new(),
            announced: HashSet::new(),
            events,
        }
    }

    /// Changes what we know about a computer. If it is worth showing and
    /// something changed, the matching event is sent.
    pub(crate) fn update(&mut self, address: IpAddr, change: impl FnOnce(&mut DiscoveredHost)) {
        let host = self
            .hosts
            .entry(address)
            .or_insert_with(|| DiscoveredHost::new(address));
        let before = host.clone();
        change(host);
        if *host == before || !host.is_interesting() {
            return;
        }
        let event = if self.announced.insert(address) {
            DiscoveryEvent::Found(host.clone())
        } else {
            DiscoveryEvent::Updated(host.clone())
        };
        // Nobody listening any more is not an error.
        let _ = self.events.send(event);
    }

    /// What we know about a computer (even if it was not announced yet).
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
}
