//! Works out which nearby addresses are worth checking.

use std::collections::HashSet;
use std::net::{IpAddr, Ipv4Addr};

/// Smallest group of addresses we ever scan is the /24 around our own address.
const MAX_SCAN_PREFIX: u8 = 24;

/// Everything we learned about this computer's network connections.
#[derive(Debug, Default)]
pub(crate) struct LocalNetworks {
    /// Every address that belongs to this computer.
    pub own: HashSet<IpAddr>,
    /// The groups of addresses that should be scanned.
    pub ranges: Vec<ScanRange>,
}

/// One group of addresses to scan, next to this computer.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ScanRange {
    pub network: Ipv4Addr,
    pub hosts: Vec<Ipv4Addr>,
}

/// Looks at the network connections of this computer.
pub(crate) fn local_networks() -> LocalNetworks {
    let interfaces = match if_addrs::get_if_addrs() {
        Ok(interfaces) => interfaces,
        Err(err) => {
            log::warn!("could not list network interfaces: {err}");
            return LocalNetworks::default();
        }
    };
    let mut result = LocalNetworks::default();
    let mut seen_networks = HashSet::new();
    for interface in interfaces {
        result.own.insert(interface.ip());
        let if_addrs::IfAddr::V4(v4) = &interface.addr else {
            continue;
        };
        if interface.is_loopback()
            || !interface.is_oper_up()
            || interface.is_p2p
            || is_virtual_interface(&interface.name)
            || !is_scannable_address(v4.ip)
        {
            log::debug!("not scanning interface {}", interface.name);
            continue;
        }
        match scan_range(v4.ip, v4.prefixlen) {
            Some(range) if seen_networks.insert(range.network) => result.ranges.push(range),
            _ => {}
        }
    }
    result
}

/// Only private home / office addresses are scanned, never public ones.
pub(crate) fn is_scannable_address(address: Ipv4Addr) -> bool {
    address.is_private()
}

/// Can a name lookup result be a neighbour? Public IPv4 addresses are not
/// trusted here (a lookup server could hand them out); IPv6 is accepted.
pub(crate) fn is_local_address(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(v4) => v4.is_private() || v4.is_link_local(),
        IpAddr::V6(v6) => !v6.is_loopback() && !v6.is_unspecified() && !v6.is_multicast(),
    }
}

/// The addresses to scan for a computer at `address` in a network with the
/// given prefix length: the /24 around it (or the whole network when it is
/// smaller), without the network and broadcast addresses and without
/// ourselves. At most 254 addresses.
pub(crate) fn scan_range(address: Ipv4Addr, prefix_len: u8) -> Option<ScanRange> {
    let prefix = prefix_len.clamp(MAX_SCAN_PREFIX, 32);
    if prefix >= 31 {
        return None;
    }
    let mask = u32::MAX << (32 - prefix);
    let own = u32::from(address);
    let network = own & mask;
    let broadcast = network | !mask;
    let hosts = (network + 1..broadcast)
        .filter(|&host| host != own)
        .map(Ipv4Addr::from)
        .collect();
    Some(ScanRange {
        network: Ipv4Addr::from(network),
        hosts,
    })
}

/// Names that belong to virtual connections (containers, tunnels, bridges,
/// virtual machines) which never hold real neighbours.
pub(crate) fn is_virtual_interface(name: &str) -> bool {
    /// Short names followed only by a number, like `lo0` or `utun4`.
    const NUMBERED: &[&str] = &[
        "lo", "ap", "gif", "stf", "tun", "tap", "wg", "ppp", "anpi", "awdl", "llw", "utun",
        "bridge", "vmnet", "vnic",
    ];
    /// Names that start with one of these.
    const PREFIXES: &[&str] = &[
        "docker",
        "br-",
        "veth",
        "virbr",
        "vboxnet",
        "tailscale",
        "zt",
        "cni",
        "flannel",
        "kube",
        "lxc",
        "lxd",
        "podman",
    ];
    /// Words that appear in Windows adapter names.
    const WORDS: &[&str] = &[
        "virtual",
        "vmware",
        "vethernet",
        "hyper-v",
        "loopback",
        "wsl",
        "zerotier",
        "bluetooth",
        "tap-windows",
        "npcap",
        "vpn",
        "tailscale",
    ];

    let lower = name.to_ascii_lowercase();
    NUMBERED.iter().any(|base| {
        lower
            .strip_prefix(base)
            .is_some_and(|rest| rest.bytes().all(|b| b.is_ascii_digit()))
    }) || PREFIXES.iter().any(|prefix| lower.starts_with(prefix))
        || WORDS.iter().any(|word| lower.contains(word))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(a: u8, b: u8, c: u8, d: u8) -> Ipv4Addr {
        Ipv4Addr::new(a, b, c, d)
    }

    #[test]
    fn scans_the_slash_24_without_ourselves() {
        let range = scan_range(ip(192, 0, 2, 17), 24).unwrap();
        assert_eq!(range.network, ip(192, 0, 2, 0));
        assert_eq!(range.hosts.len(), 253);
        assert!(!range.hosts.contains(&ip(192, 0, 2, 0)));
        assert!(!range.hosts.contains(&ip(192, 0, 2, 255)));
        assert!(!range.hosts.contains(&ip(192, 0, 2, 17)));
        assert!(range.hosts.contains(&ip(192, 0, 2, 1)));
        assert!(range.hosts.contains(&ip(192, 0, 2, 254)));
    }

    #[test]
    fn never_scans_more_than_a_slash_24() {
        let range = scan_range(ip(10, 1, 2, 3), 16).unwrap();
        assert_eq!(range.network, ip(10, 1, 2, 0));
        assert!(range.hosts.len() <= 254);
        let range = scan_range(ip(10, 1, 2, 3), 0).unwrap();
        assert_eq!(range.network, ip(10, 1, 2, 0));
    }

    #[test]
    fn small_networks_stay_small() {
        let range = scan_range(ip(10, 0, 0, 5), 28).unwrap();
        assert_eq!(range.network, ip(10, 0, 0, 0));
        assert_eq!(range.hosts.len(), 13);
        assert!(range.hosts.iter().all(|h| h.octets()[3] < 15));
    }

    #[test]
    fn tiny_networks_are_skipped() {
        assert_eq!(scan_range(ip(10, 0, 0, 5), 31), None);
        assert_eq!(scan_range(ip(10, 0, 0, 5), 32), None);
    }

    #[test]
    fn only_private_addresses_are_scanned() {
        assert!(is_scannable_address(ip(10, 1, 2, 3)));
        assert!(is_scannable_address(ip(172, 16, 0, 9)));
        assert!(is_scannable_address(ip(192, 168, 4, 2)));
        assert!(!is_scannable_address(ip(169, 254, 3, 4)));
        assert!(!is_scannable_address(ip(127, 0, 0, 1)));
        assert!(!is_scannable_address(ip(8, 8, 8, 8)));
        assert!(!is_scannable_address(ip(100, 64, 0, 1)));
    }

    #[test]
    fn name_lookup_results_must_be_neighbours() {
        assert!(is_local_address(IpAddr::V4(ip(192, 168, 1, 5))));
        assert!(is_local_address(IpAddr::V4(ip(169, 254, 1, 5))));
        assert!(!is_local_address(IpAddr::V4(ip(8, 8, 4, 4))));
        assert!(!is_local_address(IpAddr::V4(ip(127, 0, 0, 1))));
        assert!(is_local_address("fd00::5".parse().unwrap()));
        assert!(!is_local_address("::1".parse().unwrap()));
    }

    #[test]
    fn spots_virtual_interfaces() {
        for name in [
            "lo",
            "lo0",
            "utun4",
            "awdl0",
            "llw0",
            "bridge0",
            "gif0",
            "stf0",
            "ap1",
            "docker0",
            "br-1a2b3c",
            "veth12ab",
            "virbr0",
            "tun0",
            "tap1",
            "wg0",
            "tailscale0",
            "vmnet8",
            "vboxnet0",
            "vEthernet (WSL)",
            "VMware Network Adapter VMnet8",
            "Loopback Pseudo-Interface 1",
        ] {
            assert!(is_virtual_interface(name), "{name} should be virtual");
        }
    }

    #[test]
    fn keeps_real_interfaces() {
        for name in [
            "en0",
            "en5",
            "eth0",
            "wlan0",
            "enp3s0",
            "wlp2s0",
            "eno1",
            "Wi-Fi",
            "Ethernet",
            "Local Area Connection",
            "Ethernet 2",
        ] {
            assert!(!is_virtual_interface(name), "{name} should be real");
        }
    }
}
