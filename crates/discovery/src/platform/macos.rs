//! macOS: parses the output of `arp -an`.

use std::net::Ipv4Addr;

use super::ArpEntry;
use crate::identify::parse_mac;

/// Runs `arp -an` and parses it.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(super) fn read() -> Vec<ArpEntry> {
    match std::process::Command::new("/usr/sbin/arp")
        .arg("-an")
        .output()
    {
        Ok(output) => parse(&String::from_utf8_lossy(&output.stdout)),
        Err(err) => {
            log::warn!("could not run arp: {err}");
            Vec::new()
        }
    }
}

/// Parses lines like `? (192.0.2.1) at b8:27:eb:1:2:3 on en0 ifscope [ethernet]`.
/// Note that macOS leaves out leading zeros in the hardware address.
pub(super) fn parse(text: &str) -> Vec<ArpEntry> {
    text.lines().filter_map(parse_line).collect()
}

fn parse_line(line: &str) -> Option<ArpEntry> {
    let open = line.find('(')?;
    let close = open + line[open..].find(')')?;
    let ip: Ipv4Addr = line[open + 1..close].parse().ok()?;
    let rest = line[close + 1..].trim_start().strip_prefix("at ")?;
    let mac = parse_mac(rest.split_whitespace().next()?)?;
    ArpEntry::new(ip, mac)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
? (192.0.2.1) at 0:11:22:33:44:55 on en0 ifscope [ethernet]
? (192.0.2.23) at b8:27:eb:1:2:3 on en0 ifscope [ethernet]
? (192.0.2.40) at (incomplete) on en0 ifscope [ethernet]
? (192.0.2.255) at ff:ff:ff:ff:ff:ff on en0 ifscope [ethernet]
? (224.0.0.251) at 1:0:5e:0:0:fb on en0 ifscope permanent [ethernet]
nonsense line
";

    #[test]
    fn parses_sample_output() {
        let entries = parse(SAMPLE);
        assert_eq!(
            entries,
            vec![
                ArpEntry {
                    ip: Ipv4Addr::new(192, 0, 2, 1),
                    mac: [0x00, 0x11, 0x22, 0x33, 0x44, 0x55]
                },
                ArpEntry {
                    ip: Ipv4Addr::new(192, 0, 2, 23),
                    mac: [0xB8, 0x27, 0xEB, 1, 2, 3]
                },
            ]
        );
    }

    #[test]
    fn empty_output_gives_nothing() {
        assert!(parse("").is_empty());
    }
}
