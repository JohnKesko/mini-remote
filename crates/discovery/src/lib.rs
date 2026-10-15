//! Finds computers offering screen sharing on the local network.
//!
//! Several methods run at the same time and their results are merged by
//! address:
//!
//! 1. Bonjour / mDNS announcements (VNC, secure shell, "workstation").
//! 2. A quick check of the nearby addresses (the /24 around this computer)
//!    for VNC and secure shell, reading the VNC greeting to be sure.
//! 3. Looking up host names, including `raspberrypi.local`.
//! 4. Spotting Raspberry Pis by the maker part of their hardware address
//!    (read from the ARP table) and, more weakly, by their name.
//!
//! Only private network addresses are ever scanned, and nothing is sent to a
//! VNC server: the connection is closed right after reading its greeting.

use std::net::IpAddr;
use std::time::Duration;

use tokio::sync::mpsc;

mod banner;
mod engine;
mod identify;
mod mdns;
mod platform;
mod scan;
mod subnet;
mod table;

/// One computer found on the network. Computers are told apart by address.
#[derive(Clone, Debug, PartialEq)]
pub struct DiscoveredHost {
    /// The network address.
    pub address: IpAddr,
    /// A name that can be used instead of the address, such as `raspberrypi.local`.
    pub hostname: Option<String>,
    /// Ports that answered with a VNC greeting, lowest first.
    pub vnc_ports: Vec<u16>,
    /// VNC protocol version of the lowest VNC port, such as `3.8`.
    pub rfb_version: Option<String>,
    /// Port 22 is open (needed for file transfer and secure tunnels).
    pub ssh: bool,
    /// Looks like a Raspberry Pi (by hardware address, or weaker, by name).
    pub is_raspberry_pi: bool,
    /// The name the computer gave to its mDNS announcement, if any.
    pub service_name: Option<String>,
}

/// What discovery reports while it runs.
#[derive(Clone, Debug, PartialEq)]
pub enum DiscoveryEvent {
    /// A computer was seen for the first time.
    Found(DiscoveredHost),
    /// More was learned about a computer that was already reported.
    Updated(DiscoveredHost),
    /// An address reported earlier turned out to be another address of a
    /// computer that is listed under its IPv4 address. Drop it from the list.
    Removed(IpAddr),
    /// Discovery is over. The channel closes after this.
    Finished,
}

/// Settings for [`discover`].
#[derive(Clone, Debug)]
pub struct DiscoveryOptions {
    /// Check the nearby addresses of every network connection.
    /// Computers announced over mDNS are still checked when this is off.
    pub scan_subnet: bool,
    /// Ports to look for VNC on.
    pub vnc_ports: Vec<u16>,
    /// How long to wait for a single connection and for a VNC greeting.
    pub connect_timeout: Duration,
    /// Upper limit for the whole discovery.
    pub total_timeout: Duration,
}

impl Default for DiscoveryOptions {
    fn default() -> Self {
        Self {
            scan_subnet: true,
            vnc_ports: vec![5900, 5901],
            connect_timeout: Duration::from_millis(400),
            total_timeout: Duration::from_secs(8),
        }
    }
}

/// Starts discovery on the current tokio runtime. Events arrive on the
/// returned channel until [`DiscoveryEvent::Finished`]. Dropping the receiver
/// stops the discovery early.
///
/// Called outside a tokio runtime, it only logs an error and reports
/// `Finished` at once.
pub fn discover(opts: DiscoveryOptions) -> mpsc::UnboundedReceiver<DiscoveryEvent> {
    let (events, receiver) = mpsc::unbounded_channel();
    let Ok(runtime) = tokio::runtime::Handle::try_current() else {
        log::error!("discover() must be called inside a tokio runtime");
        let _ = events.send(DiscoveryEvent::Finished);
        return receiver;
    };
    runtime.spawn(async move {
        // Run in a separate task so that `Finished` is sent even after a panic.
        let worker = tokio::spawn(engine::run(opts, events.clone()));
        if let Err(err) = worker.await {
            log::error!("discovery stopped unexpectedly: {err}");
        }
        let _ = events.send(DiscoveryEvent::Finished);
    });
    receiver
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_options() {
        let opts = DiscoveryOptions::default();
        assert!(opts.scan_subnet);
        assert_eq!(opts.vnc_ports, vec![5900, 5901]);
        assert_eq!(opts.connect_timeout, Duration::from_millis(400));
        assert_eq!(opts.total_timeout, Duration::from_secs(8));
    }

    #[test]
    fn without_a_runtime_it_finishes_at_once() {
        let mut receiver = discover(DiscoveryOptions::default());
        assert_eq!(receiver.try_recv().ok(), Some(DiscoveryEvent::Finished));
    }

    #[tokio::test]
    async fn finishes_within_the_time_limit() {
        let opts = DiscoveryOptions {
            scan_subnet: false,
            total_timeout: Duration::from_millis(600),
            ..DiscoveryOptions::default()
        };
        let started = std::time::Instant::now();
        let mut receiver = discover(opts);
        let mut last = None;
        while let Some(event) = receiver.recv().await {
            last = Some(event);
        }
        assert_eq!(last, Some(DiscoveryEvent::Finished));
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
