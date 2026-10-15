//! Helpers that decide whether a computer is a Raspberry Pi, and tidy up names.

use std::net::IpAddr;

/// A hardware (MAC) address.
pub(crate) type Mac = [u8; 6];

/// The first three bytes of hardware addresses registered to Raspberry Pi
/// (checked against the public IEEE list: Raspberry Pi Foundation and
/// Raspberry Pi Trading Ltd).
const RASPBERRY_PI_PREFIXES: [[u8; 3]; 8] = [
    [0xB8, 0x27, 0xEB],
    [0xDC, 0xA6, 0x32],
    [0xE4, 0x5F, 0x01],
    [0x28, 0xCD, 0xC1],
    [0xD8, 0x3A, 0xDD],
    [0x2C, 0xCF, 0x67],
    [0x88, 0xA2, 0x9E],
    [0x98, 0xFE, 0x54],
];

/// True when the hardware address belongs to a Raspberry Pi maker block.
pub(crate) fn is_raspberry_pi_mac(mac: &Mac) -> bool {
    RASPBERRY_PI_PREFIXES
        .iter()
        .any(|prefix| mac[..3] == prefix[..])
}

/// Reads a hardware address such as `b8:27:eb:01:02:03`, `B8-27-EB-01-02-03`
/// or the short macOS form `b8:27:eb:1:2:3`.
pub(crate) fn parse_mac(text: &str) -> Option<Mac> {
    let mut mac = [0u8; 6];
    let mut parts = text.trim().split([':', '-']);
    for byte in &mut mac {
        let part = parts.next()?;
        if part.is_empty() || part.len() > 2 || !part.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        *byte = u8::from_str_radix(part, 16).ok()?;
    }
    if parts.next().is_some() {
        return None;
    }
    Some(mac)
}

/// True for a real single-computer hardware address: not empty, not the
/// broadcast address and not a group (multicast) address.
pub(crate) fn is_unicast_mac(mac: &Mac) -> bool {
    *mac != [0u8; 6] && mac[0] & 1 == 0
}

/// A weaker hint: does the first part of the host name look like a Pi name
/// (`raspberrypi`, `raspi4`, `pi`, `pi4`, `pi-kitchen`)?
pub(crate) fn hostname_hints_raspberry_pi(name: &str) -> bool {
    let lower = name.trim().trim_end_matches('.').to_ascii_lowercase();
    let label = lower.split('.').next().unwrap_or_default();
    if label.contains("raspberry") || label.contains("raspi") {
        return true;
    }
    match label.strip_prefix("pi") {
        Some("") => true,
        Some(rest) => rest.starts_with(|c: char| c.is_ascii_digit() || c == '-' || c == '_'),
        None => false,
    }
}

/// Removes the trailing dot and rejects empty names and plain addresses.
pub(crate) fn clean_hostname(name: &str) -> Option<String> {
    let name = name.trim().trim_end_matches('.');
    if name.is_empty() || name.parse::<IpAddr>().is_ok() {
        return None;
    }
    Some(name.to_string())
}

/// True for names that were found through Bonjour / mDNS (`something.local`).
pub(crate) fn is_local_name(name: &str) -> bool {
    name.to_ascii_lowercase().ends_with(".local")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_every_known_raspberry_pi_prefix() {
        for prefix in RASPBERRY_PI_PREFIXES {
            let mac = [prefix[0], prefix[1], prefix[2], 0x12, 0x34, 0x56];
            assert!(is_raspberry_pi_mac(&mac));
        }
    }

    #[test]
    fn rejects_other_vendors() {
        assert!(!is_raspberry_pi_mac(&[0x00, 0x1A, 0x2B, 0x3C, 0x4D, 0x5E]));
        assert!(!is_raspberry_pi_mac(&[0xB8, 0x27, 0xEC, 0, 0, 1]));
    }

    #[test]
    fn parses_mac_in_common_notations() {
        let expected = [0xB8, 0x27, 0xEB, 0x01, 0x02, 0x03];
        assert_eq!(parse_mac("b8:27:eb:01:02:03"), Some(expected));
        assert_eq!(parse_mac("B8-27-EB-01-02-03"), Some(expected));
        assert_eq!(parse_mac("b8:27:eb:1:2:3"), Some(expected));
    }

    #[test]
    fn rejects_bad_mac_text() {
        assert_eq!(parse_mac("(incomplete)"), None);
        assert_eq!(parse_mac("b8:27:eb:01:02"), None);
        assert_eq!(parse_mac("b8:27:eb:01:02:03:04"), None);
        assert_eq!(parse_mac("b8:27:eb:01:02:zz"), None);
        assert_eq!(parse_mac("b8:27:eb:001:02:03"), None);
        assert_eq!(parse_mac(""), None);
    }

    #[test]
    fn unicast_check_skips_empty_and_group_addresses() {
        assert!(is_unicast_mac(&[0xB8, 0x27, 0xEB, 1, 2, 3]));
        assert!(!is_unicast_mac(&[0; 6]));
        assert!(!is_unicast_mac(&[0xFF; 6]));
        assert!(!is_unicast_mac(&[0x01, 0x00, 0x5E, 0, 0, 0xFB]));
    }

    #[test]
    fn host_name_hints() {
        assert!(hostname_hints_raspberry_pi("raspberrypi.local"));
        assert!(hostname_hints_raspberry_pi("RaspberryPi"));
        assert!(hostname_hints_raspberry_pi("raspberrypi.example.lan."));
        assert!(hostname_hints_raspberry_pi("raspi4"));
        assert!(hostname_hints_raspberry_pi("pi"));
        assert!(hostname_hints_raspberry_pi("pi4.local"));
        assert!(hostname_hints_raspberry_pi("pi-zero"));
        assert!(!hostname_hints_raspberry_pi("pixel-7"));
        assert!(!hostname_hints_raspberry_pi("pihole"));
        assert!(!hostname_hints_raspberry_pi("laptop.pi.example"));
        assert!(!hostname_hints_raspberry_pi(""));
    }

    #[test]
    fn tidies_host_names() {
        assert_eq!(clean_hostname("host.local."), Some("host.local".into()));
        assert_eq!(clean_hostname("  "), None);
        assert_eq!(clean_hostname("192.0.2.7"), None);
        assert_eq!(clean_hostname("."), None);
    }

    #[test]
    fn recognises_local_names() {
        assert!(is_local_name("box.local"));
        assert!(is_local_name("Box.LOCAL"));
        assert!(!is_local_name("box.example.lan"));
    }
}
