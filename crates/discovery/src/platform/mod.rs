//! The parts that differ between operating systems: reading the list of
//! neighbouring computers (the "ARP table") together with their hardware
//! addresses.

use std::net::Ipv4Addr;

use crate::identify::{Mac, is_unicast_mac};

#[cfg(any(target_os = "linux", test))]
mod linux;
#[cfg(any(target_os = "macos", test))]
mod macos;
#[cfg(any(windows, test))]
mod windows;

/// One line of the ARP table: a neighbour and its hardware address.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ArpEntry {
    pub ip: Ipv4Addr,
    pub mac: Mac,
}

impl ArpEntry {
    /// Builds an entry, dropping broadcast, multicast and empty ones.
    pub(crate) fn new(ip: Ipv4Addr, mac: Mac) -> Option<Self> {
        let usable_ip =
            !ip.is_unspecified() && !ip.is_loopback() && !ip.is_multicast() && !ip.is_broadcast();
        (usable_ip && is_unicast_mac(&mac)).then_some(Self { ip, mac })
    }
}

/// Reads the ARP table of this computer. Blocking; returns nothing when the
/// platform is unknown or the table cannot be read.
pub(crate) fn read_arp_table() -> Vec<ArpEntry> {
    #[cfg(target_os = "macos")]
    {
        macos::read()
    }
    #[cfg(target_os = "linux")]
    {
        linux::read()
    }
    #[cfg(windows)]
    {
        windows::read()
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
    {
        Vec::new()
    }
}
