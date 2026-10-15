//! The coordinator: runs all discovery methods at the same time and merges
//! what they learn into one list of computers.

use std::collections::HashSet;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Semaphore;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::task::JoinSet;
use tokio::time::{Instant, MissedTickBehavior, interval, sleep_until, timeout, timeout_at};

use crate::identify::{is_local_name, is_raspberry_pi_mac};
use crate::mdns::{self, Announcement, MdnsBrowser, ServiceKind};
use crate::platform::{self, ArpEntry};
use crate::scan::{self, PortKind, PortState};
use crate::subnet;
use crate::table::HostTable;
use crate::{DiscoveryEvent, DiscoveryOptions};

/// The secure shell port.
const SSH_PORT: u16 = 22;
/// How many connection attempts run at the same time during the scan.
const MAX_PARALLEL_CONNECTS: usize = 64;
/// Names that Raspberry Pi OS gives to a new Pi. Tried directly.
const WELL_KNOWN_NAMES: [&str; 1] = ["raspberrypi.local"];
/// Longest wait for finding the name that belongs to an address.
const REVERSE_LOOKUP_TIMEOUT: Duration = Duration::from_millis(2500);
/// Longest wait for finding the address that belongs to a name.
const FORWARD_LOOKUP_TIMEOUT: Duration = Duration::from_secs(3);
/// Larger time limits are shortened to this (it also avoids overflow).
const LONGEST_TOTAL_TIMEOUT: Duration = Duration::from_secs(3600);
/// Time kept free at the end for reading the ARP table one last time.
const FINAL_PASS_RESERVE: Duration = Duration::from_millis(250);
/// We never stop earlier than this, so slow mDNS answers can arrive.
const SETTLE_TIME: Duration = Duration::from_secs(4);
/// ...and only when no mDNS answer came in during this time.
const MDNS_QUIET_TIME: Duration = Duration::from_secs(1);
/// Number of instant "unreachable" answers that suggests a blocked network.
const UNREACHABLE_WARNING_COUNT: usize = 20;

/// Runs one discovery. Returns when finished or when the time is up.
pub(crate) async fn run(opts: DiscoveryOptions, events: UnboundedSender<DiscoveryEvent>) {
    let start = Instant::now();
    let total = opts.total_timeout.min(LONGEST_TOTAL_TIMEOUT);
    let deadline = start + total;
    let reserve = FINAL_PASS_RESERVE.min(total / 4);
    let stop = deadline.checked_sub(reserve).unwrap_or(start);
    let mut engine = Engine::new(opts, events);
    engine.start();
    engine.work_until(start, stop).await;
    engine.finish(deadline).await;
}

/// Results sent back by the background jobs.
enum Message {
    /// One port was tried.
    Probe {
        ip: IpAddr,
        port: u16,
        kind: PortKind,
        state: PortState,
    },
    /// The system resolver knows a name for this address.
    Name { ip: IpAddr, name: String },
    /// A well-known name turned out to have this address.
    Resolved { name: String, ip: IpAddr },
    /// The ARP table was read.
    Arp(Vec<ArpEntry>),
}

struct Engine {
    opts: DiscoveryOptions,
    table: HostTable,
    /// Addresses of this computer (never reported).
    own: HashSet<IpAddr>,
    /// Ports that were already tried (or are being tried).
    probed: HashSet<(IpAddr, u16)>,
    /// Addresses for which a name lookup was started.
    name_requested: HashSet<IpAddr>,
    /// Names whose addresses were looked up (so one computer seen at several
    /// addresses gets its name on all of them and is listed once).
    names_resolved: HashSet<String>,
    /// Addresses that answered in some way, so they exist right now.
    alive: HashSet<IpAddr>,
    /// How many tries failed instantly with "unreachable".
    unreachable: usize,
    /// No new background jobs are started once this is set.
    closing: bool,
    jobs: JoinSet<()>,
    messages: UnboundedReceiver<Message>,
    message_sender: UnboundedSender<Message>,
    announcements: UnboundedReceiver<Announcement>,
    announcement_sender: UnboundedSender<Announcement>,
    mdns: Option<MdnsBrowser>,
}

impl Engine {
    fn new(opts: DiscoveryOptions, events: UnboundedSender<DiscoveryEvent>) -> Self {
        let (message_sender, messages) = unbounded_channel();
        let (announcement_sender, announcements) = unbounded_channel();
        Self {
            opts,
            table: HostTable::new(events),
            own: HashSet::new(),
            probed: HashSet::new(),
            name_requested: HashSet::new(),
            names_resolved: HashSet::new(),
            alive: HashSet::new(),
            unreachable: 0,
            closing: false,
            jobs: JoinSet::new(),
            messages,
            message_sender,
            announcements,
            announcement_sender,
            mdns: None,
        }
    }

    /// Starts all the discovery methods.
    fn start(&mut self) {
        let networks = subnet::local_networks();
        self.own = networks.own;
        if self.opts.scan_subnet {
            let targets: HashSet<Ipv4Addr> = networks
                .ranges
                .iter()
                .flat_map(|range| range.hosts.iter().copied())
                .collect();
            log::debug!("scanning {} addresses", targets.len());
            self.start_sweep(targets);
        }
        self.mdns = mdns::start(self.announcement_sender.clone());
        for name in WELL_KNOWN_NAMES {
            self.start_forward_lookup(name.to_string());
        }
    }

    /// Handles results until the time is up, everything is done, or nobody
    /// listens any more.
    async fn work_until(&mut self, start: Instant, stop: Instant) {
        let mut tick = interval(Duration::from_millis(250));
        tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let mut last_announcement = start;
        loop {
            tokio::select! {
                _ = sleep_until(stop) => return,
                Some(message) = self.messages.recv() => self.on_message(message),
                Some(announcement) = self.announcements.recv() => {
                    last_announcement = Instant::now();
                    self.on_announcement(announcement);
                }
                Some(result) = self.jobs.join_next() => {
                    if result.is_err_and(|err| err.is_panic()) {
                        log::error!("a discovery job panicked");
                    }
                }
                _ = tick.tick() => {
                    let now = Instant::now();
                    if self.table.is_closed() {
                        return;
                    }
                    if self.jobs.is_empty()
                        && now - start >= SETTLE_TIME
                        && now - last_announcement >= MDNS_QUIET_TIME
                    {
                        return;
                    }
                }
            }
        }
    }

    /// Stops all work, handles what is left and reads the ARP table a last
    /// time (to find hardware addresses of computers found late).
    async fn finish(mut self, deadline: Instant) {
        self.closing = true;
        self.jobs.abort_all();
        while let Ok(message) = self.messages.try_recv() {
            self.on_message(message);
        }
        while let Ok(announcement) = self.announcements.try_recv() {
            self.on_announcement(announcement);
        }
        let table = tokio::task::spawn_blocking(platform::read_arp_table);
        if let Ok(Ok(entries)) = timeout_at(deadline, table).await {
            self.on_arp(entries);
        }
        if self.alive.is_empty() && self.unreachable >= UNREACHABLE_WARNING_COUNT {
            log::warn!(
                "many addresses were unreachable at once; the system may be \
                 blocking access to the local network for this program"
            );
        }
    }

    // ----- Starting background jobs -----

    /// Checks all addresses for VNC and secure shell, most important port
    /// first, with limited parallelism. Reads the ARP table when done.
    fn start_sweep(&mut self, targets: HashSet<Ipv4Addr>) {
        if targets.is_empty() {
            return;
        }
        let mut targets: Vec<Ipv4Addr> = targets.into_iter().collect();
        targets.sort();
        let ports = self.sweep_ports();
        for &ip in &targets {
            for &(port, _) in &ports {
                self.probed.insert((IpAddr::V4(ip), port));
            }
        }
        let messages = self.message_sender.clone();
        let wait = self.opts.connect_timeout;
        self.jobs.spawn(async move {
            let permits = Arc::new(Semaphore::new(MAX_PARALLEL_CONNECTS));
            let mut probes = JoinSet::new();
            for (port, kind) in ports {
                for &ip in &targets {
                    let permits = Arc::clone(&permits);
                    let messages = messages.clone();
                    probes.spawn(async move {
                        let Ok(_permit) = permits.acquire_owned().await else {
                            return;
                        };
                        probe_and_report(IpAddr::V4(ip), port, kind, wait, &messages).await;
                    });
                }
            }
            while probes.join_next().await.is_some() {}
            let entries = tokio::task::spawn_blocking(platform::read_arp_table)
                .await
                .unwrap_or_default();
            let _ = messages.send(Message::Arp(entries));
        });
    }

    /// The ports to scan, in order: first VNC port, secure shell, other VNC ports.
    fn sweep_ports(&self) -> Vec<(u16, PortKind)> {
        let mut vnc = self.vnc_ports().into_iter();
        let mut ports = Vec::new();
        ports.extend(vnc.next().map(|port| (port, PortKind::Vnc)));
        ports.push((SSH_PORT, PortKind::Ssh));
        ports.extend(vnc.map(|port| (port, PortKind::Vnc)));
        ports
    }

    /// The configured VNC ports without duplicates (and without port 22).
    fn vnc_ports(&self) -> Vec<u16> {
        let mut seen = HashSet::new();
        self.opts
            .vnc_ports
            .iter()
            .copied()
            .filter(|&port| port != SSH_PORT && seen.insert(port))
            .collect()
    }

    /// Tries one port once (no parallelism limit; used for few addresses).
    fn probe_once(&mut self, ip: IpAddr, port: u16, kind: PortKind) {
        if self.closing || !self.probed.insert((ip, port)) {
            return;
        }
        let messages = self.message_sender.clone();
        let wait = self.opts.connect_timeout;
        self.jobs.spawn(async move {
            probe_and_report(ip, port, kind, wait, &messages).await;
        });
    }

    /// Tries the usual ports on an address that was found another way.
    fn probe_usual_ports(&mut self, ip: IpAddr) {
        for port in self.vnc_ports() {
            self.probe_once(ip, port, PortKind::Vnc);
        }
        self.probe_once(ip, SSH_PORT, PortKind::Ssh);
    }

    /// Asks the system for the name of an address, unless we know one.
    fn request_name(&mut self, ip: IpAddr) {
        if self.closing
            || self
                .table
                .get(ip)
                .is_some_and(|host| host.hostname.is_some())
            || !self.name_requested.insert(ip)
        {
            return;
        }
        let messages = self.message_sender.clone();
        self.jobs.spawn(async move {
            let lookup = tokio::task::spawn_blocking(move || dns_lookup::lookup_addr(&ip));
            match timeout(REVERSE_LOOKUP_TIMEOUT, lookup).await {
                Ok(Ok(Ok(name))) => {
                    let _ = messages.send(Message::Name { ip, name });
                }
                Ok(Ok(Err(err))) => log::debug!("no name found for {ip}: {err}"),
                Ok(Err(err)) => log::warn!("name lookup failed: {err}"),
                Err(_) => log::debug!("name lookup for {ip} took too long"),
            }
        });
    }

    /// Asks the system which address belongs to a name such as `raspberrypi.local`.
    fn start_forward_lookup(&mut self, name: String) {
        if self.closing || !self.names_resolved.insert(name.to_ascii_lowercase()) {
            return;
        }
        let messages = self.message_sender.clone();
        self.jobs.spawn(async move {
            let host = name.clone();
            let lookup = tokio::task::spawn_blocking(move || {
                dns_lookup::lookup_host(&host).map(Iterator::collect::<Vec<_>>)
            });
            match timeout(FORWARD_LOOKUP_TIMEOUT, lookup).await {
                Ok(Ok(Ok(addresses))) => {
                    // Every IPv4 address (a Pi on Wi-Fi and cable has two).
                    for ip in mdns::pick_addresses(addresses.into_iter()) {
                        let _ = messages.send(Message::Resolved {
                            name: name.clone(),
                            ip,
                        });
                    }
                }
                Ok(Ok(Err(err))) => log::debug!("{name} did not resolve: {err}"),
                Ok(Err(err)) => log::warn!("name lookup failed: {err}"),
                Err(_) => log::debug!("looking up {name} took too long"),
            }
        });
    }

    // ----- Handling results -----

    fn on_message(&mut self, message: Message) {
        match message {
            Message::Probe {
                ip,
                port,
                kind,
                state,
            } => self.on_probe(ip, port, kind, state),
            Message::Name { ip, name } => {
                self.table.update(ip, |host| host.set_hostname(&name));
                self.resolve_local_name(&name);
            }
            Message::Resolved { name, ip } => self.on_resolved(&name, ip),
            Message::Arp(entries) => self.on_arp(entries),
        }
    }

    fn on_probe(&mut self, ip: IpAddr, port: u16, kind: PortKind, state: PortState) {
        match state {
            PortState::Vnc(version) => {
                self.alive.insert(ip);
                self.table
                    .update(ip, |host| host.add_vnc_port(port, &version));
                self.request_name(ip);
            }
            PortState::Open if kind == PortKind::Ssh => {
                self.alive.insert(ip);
                self.table.update(ip, |host| host.ssh = true);
                self.request_name(ip);
            }
            PortState::Open => {
                self.alive.insert(ip);
                log::debug!("port {port} is open on {ip} but is not VNC");
            }
            PortState::Refused => {
                self.alive.insert(ip);
            }
            PortState::Unreachable => self.unreachable += 1,
            PortState::NoAnswer => {}
        }
    }

    fn on_resolved(&mut self, name: &str, ip: IpAddr) {
        if self.own.contains(&ip) || !subnet::is_local_address(ip) {
            return;
        }
        self.table.update(ip, |host| host.set_hostname(name));
        self.probe_usual_ports(ip);
    }

    /// Looks up the addresses of a `.local` name, so its name lands on all of them.
    fn resolve_local_name(&mut self, name: &str) {
        let name = name.trim_end_matches('.');
        if is_local_name(name) {
            self.start_forward_lookup(name.to_string());
        }
    }

    fn on_announcement(&mut self, announcement: Announcement) {
        log::debug!(
            "mDNS {:?} announcement with {} address(es)",
            announcement.kind,
            announcement.addresses.len()
        );
        for &ip in &announcement.addresses {
            if self.own.contains(&ip) {
                continue;
            }
            self.table.update(ip, |host| {
                if let Some(name) = &announcement.hostname {
                    host.set_hostname(name);
                }
                match announcement.kind {
                    ServiceKind::Rfb => host.service_name = Some(announcement.instance.clone()),
                    ServiceKind::Ssh => {
                        if announcement.port == SSH_PORT {
                            host.ssh = true;
                        }
                        host.service_name
                            .get_or_insert_with(|| announcement.instance.clone());
                    }
                    ServiceKind::Workstation => {}
                    ServiceKind::DeviceInfo => {
                        let is_pi_model = announcement
                            .model
                            .as_deref()
                            .is_some_and(|model| model.to_ascii_lowercase().contains("raspberry"));
                        if is_pi_model {
                            host.is_raspberry_pi = true;
                        }
                    }
                }
                if announcement.mac.as_ref().is_some_and(is_raspberry_pi_mac) {
                    host.is_raspberry_pi = true;
                }
            });
            if let Some(name) = &announcement.hostname {
                self.resolve_local_name(name);
            }
            if announcement.kind == ServiceKind::Rfb {
                // A VNC server may announce a port outside the usual ones.
                self.probe_once(ip, announcement.port, PortKind::Vnc);
            }
            self.probe_usual_ports(ip);
            self.request_name(ip);
        }
    }

    /// Uses the hardware addresses to spot Raspberry Pis. A table entry alone
    /// can be old, so only computers that answered us count.
    fn on_arp(&mut self, entries: Vec<ArpEntry>) {
        log::debug!(
            "ARP table has {} entries, {} from Raspberry Pi",
            entries.len(),
            entries
                .iter()
                .filter(|e| is_raspberry_pi_mac(&e.mac))
                .count()
        );
        for entry in entries {
            let ip = IpAddr::V4(entry.ip);
            if self.own.contains(&ip) || !is_raspberry_pi_mac(&entry.mac) {
                continue;
            }
            if self.alive.contains(&ip) || self.table.get(ip).is_some() {
                self.table.update(ip, |host| host.is_raspberry_pi = true);
                self.request_name(ip);
            }
        }
    }
}

/// Tries one port and reports the result.
async fn probe_and_report(
    ip: IpAddr,
    port: u16,
    kind: PortKind,
    wait: Duration,
    messages: &UnboundedSender<Message>,
) {
    let state = scan::probe(ip, port, kind, wait).await;
    let _ = messages.send(Message::Probe {
        ip,
        port,
        kind,
        state,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const PI: IpAddr = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 20));
    const OTHER: IpAddr = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 21));
    const PI_MAC: [u8; 6] = [0xB8, 0x27, 0xEB, 1, 2, 3];

    /// An engine that never starts background jobs.
    fn quiet_engine() -> (Engine, UnboundedReceiver<DiscoveryEvent>) {
        let (events, receiver) = unbounded_channel();
        let mut engine = Engine::new(DiscoveryOptions::default(), events);
        engine.closing = true;
        (engine, receiver)
    }

    fn announcement(kind: ServiceKind, port: u16) -> Announcement {
        Announcement {
            kind,
            instance: "instance".into(),
            hostname: Some("raspberrypi.local".into()),
            port,
            addresses: vec![PI],
            mac: None,
            model: None,
        }
    }

    #[tokio::test]
    async fn probe_results_become_found_and_updated() {
        let (mut engine, mut events) = quiet_engine();
        engine.on_probe(OTHER, 5900, PortKind::Vnc, PortState::Vnc("3.8".into()));
        engine.on_probe(OTHER, SSH_PORT, PortKind::Ssh, PortState::Open);
        engine.on_probe(OTHER, 5901, PortKind::Vnc, PortState::Refused);

        let Some(DiscoveryEvent::Found(found)) = events.try_recv().ok() else {
            panic!("expected Found");
        };
        assert_eq!(found.vnc_ports, vec![5900]);
        assert_eq!(found.rfb_version.as_deref(), Some("3.8"));
        assert!(!found.ssh);
        let Some(DiscoveryEvent::Updated(updated)) = events.try_recv().ok() else {
            panic!("expected Updated");
        };
        assert!(updated.ssh);
        assert!(events.try_recv().is_err());
    }

    #[tokio::test]
    async fn open_port_without_vnc_greeting_is_not_vnc() {
        let (mut engine, mut events) = quiet_engine();
        engine.on_probe(OTHER, 5900, PortKind::Vnc, PortState::Open);
        assert!(events.try_recv().is_err());
    }

    #[tokio::test]
    async fn stale_arp_entry_is_not_reported() {
        let (mut engine, mut events) = quiet_engine();
        engine.on_arp(vec![ArpEntry {
            ip: Ipv4Addr::new(192, 0, 2, 20),
            mac: PI_MAC,
        }]);
        assert!(events.try_recv().is_err());
    }

    #[tokio::test]
    async fn answering_computer_with_pi_hardware_address_is_a_pi() {
        let (mut engine, mut events) = quiet_engine();
        // Port closed, but the computer answered.
        engine.on_probe(PI, 5900, PortKind::Vnc, PortState::Refused);
        engine.on_arp(vec![ArpEntry {
            ip: Ipv4Addr::new(192, 0, 2, 20),
            mac: PI_MAC,
        }]);
        let Some(DiscoveryEvent::Found(found)) = events.try_recv().ok() else {
            panic!("expected Found");
        };
        assert!(found.is_raspberry_pi);
        assert!(found.vnc_ports.is_empty());
    }

    #[tokio::test]
    async fn own_addresses_are_never_reported() {
        let (mut engine, mut events) = quiet_engine();
        engine.own.insert(PI);
        engine.on_announcement(announcement(ServiceKind::Ssh, SSH_PORT));
        engine.on_resolved("raspberrypi.local", PI);
        assert!(events.try_recv().is_err());
    }

    #[tokio::test]
    async fn name_lookup_giving_a_public_address_is_ignored() {
        let (mut engine, mut events) = quiet_engine();
        engine.on_resolved("raspberrypi.local", IpAddr::V4(Ipv4Addr::new(8, 8, 4, 4)));
        assert!(events.try_recv().is_err());
        // 192.0.2.x is a documentation range and not private, so use a private one.
        let neighbour = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 20));
        engine.on_resolved("raspberrypi.local", neighbour);
        assert!(matches!(
            events.try_recv().ok(),
            Some(DiscoveryEvent::Found(_))
        ));
    }

    #[tokio::test]
    async fn workstation_announcement_with_pi_hardware_address() {
        let (mut engine, mut events) = quiet_engine();
        let mut note = announcement(ServiceKind::Workstation, 9);
        note.hostname = Some("kitchen.local".into());
        note.mac = Some(PI_MAC);
        engine.on_announcement(note);
        let Some(DiscoveryEvent::Found(found)) = events.try_recv().ok() else {
            panic!("expected Found");
        };
        assert!(found.is_raspberry_pi);
        assert_eq!(found.hostname.as_deref(), Some("kitchen.local"));
    }

    #[tokio::test]
    async fn rfb_and_ssh_announcements_fill_in_names() {
        let (mut engine, mut events) = quiet_engine();
        let mut ssh = announcement(ServiceKind::Ssh, SSH_PORT);
        ssh.instance = "from ssh".into();
        engine.on_announcement(ssh);
        let mut rfb = announcement(ServiceKind::Rfb, 5900);
        rfb.instance = "from rfb".into();
        engine.on_announcement(rfb);

        let Some(DiscoveryEvent::Found(found)) = events.try_recv().ok() else {
            panic!("expected Found");
        };
        assert!(found.ssh);
        assert_eq!(found.service_name.as_deref(), Some("from ssh"));
        let Some(DiscoveryEvent::Updated(updated)) = events.try_recv().ok() else {
            panic!("expected Updated");
        };
        assert_eq!(updated.service_name.as_deref(), Some("from rfb"));
    }

    #[tokio::test]
    async fn device_info_model_marks_a_pi() {
        let (mut engine, mut events) = quiet_engine();
        let mut info = announcement(ServiceKind::DeviceInfo, 0);
        info.hostname = Some("box.local".into());
        info.model = Some("Raspberry Pi 5 Model B".into());
        engine.on_announcement(info);
        let Some(DiscoveryEvent::Found(found)) = events.try_recv().ok() else {
            panic!("expected Found");
        };
        assert!(found.is_raspberry_pi);
    }

    #[test]
    fn sweep_order_and_port_list() {
        let (events, _receiver) = unbounded_channel();
        let opts = DiscoveryOptions {
            vnc_ports: vec![5900, 22, 5901, 5900, 5902],
            ..DiscoveryOptions::default()
        };
        let engine = Engine::new(opts, events);
        assert_eq!(engine.vnc_ports(), vec![5900, 5901, 5902]);
        assert_eq!(
            engine.sweep_ports(),
            vec![
                (5900, PortKind::Vnc),
                (SSH_PORT, PortKind::Ssh),
                (5901, PortKind::Vnc),
                (5902, PortKind::Vnc)
            ]
        );
    }

    #[test]
    fn no_vnc_ports_still_checks_ssh() {
        let (events, _receiver) = unbounded_channel();
        let opts = DiscoveryOptions {
            vnc_ports: vec![],
            ..DiscoveryOptions::default()
        };
        let engine = Engine::new(opts, events);
        assert_eq!(engine.sweep_ports(), vec![(SSH_PORT, PortKind::Ssh)]);
    }
}
