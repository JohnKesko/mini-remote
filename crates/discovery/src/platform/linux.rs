//! Linux: parses `/proc/net/arp`.

use std::net::Ipv4Addr;

use super::ArpEntry;
use crate::identify::parse_mac;

/// The "entry is complete" flag in the table.
const FLAG_COMPLETE: u32 = 0x2;

/// Reads `/proc/net/arp` and parses it.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(super) fn read() -> Vec<ArpEntry> {
    match std::fs::read_to_string("/proc/net/arp") {
        Ok(text) => parse(&text),
        Err(err) => {
            log::warn!("could not read the ARP table: {err}");
            Vec::new()
        }
    }
}

/// Parses lines like `192.0.2.1  0x1  0x2  b8:27:eb:01:02:03  *  eth0`.
pub(super) fn parse(text: &str) -> Vec<ArpEntry> {
    text.lines().skip(1).filter_map(parse_line).collect()
}

fn parse_line(line: &str) -> Option<ArpEntry> {
    let mut columns = line.split_whitespace();
    let ip: Ipv4Addr = columns.next()?.parse().ok()?;
    let _hardware_type = columns.next()?;
    let flags = u32::from_str_radix(columns.next()?.trim_start_matches("0x"), 16).ok()?;
    if flags & FLAG_COMPLETE == 0 {
        return None;
    }
    let mac = parse_mac(columns.next()?)?;
    ArpEntry::new(ip, mac)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
IP address       HW type     Flags       HW address            Mask     Device
192.0.2.1        0x1         0x2         00:11:22:33:44:55     *        eth0
192.0.2.23       0x1         0x2         dc:a6:32:01:02:03     *        eth0
192.0.2.40       0x1         0x0         00:00:00:00:00:00     *        eth0
192.0.2.41       0x1         0x0         aa:bb:cc:dd:ee:ff     *        eth0
";

    #[test]
    fn parses_sample_table() {
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
                    mac: [0xDC, 0xA6, 0x32, 1, 2, 3]
                },
            ]
        );
    }

    #[test]
    fn header_only_gives_nothing() {
        assert!(parse("IP address HW type Flags HW address Mask Device\n").is_empty());
    }
}
