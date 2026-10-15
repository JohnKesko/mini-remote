//! Windows: parses the output of `arp -a`.

use std::net::Ipv4Addr;

use super::ArpEntry;
use crate::identify::parse_mac;

/// Runs `arp -a` and parses it.
#[cfg_attr(not(windows), allow(dead_code))]
pub(super) fn read() -> Vec<ArpEntry> {
    let mut command = std::process::Command::new("arp");
    command.arg("-a");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // Do not flash a console window.
        command.creation_flags(0x0800_0000);
    }
    match command.output() {
        Ok(output) => parse(&String::from_utf8_lossy(&output.stdout)),
        Err(err) => {
            log::warn!("could not run arp: {err}");
            Vec::new()
        }
    }
}

/// Parses lines like `  192.0.2.1          b8-27-eb-01-02-03     dynamic`.
pub(super) fn parse(text: &str) -> Vec<ArpEntry> {
    text.lines().filter_map(parse_line).collect()
}

fn parse_line(line: &str) -> Option<ArpEntry> {
    let mut columns = line.split_whitespace();
    let ip: Ipv4Addr = columns.next()?.parse().ok()?;
    let mac = parse_mac(columns.next()?)?;
    ArpEntry::new(ip, mac)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "
Interface: 192.0.2.50 --- 0x4
  Internet Address      Physical Address      Type
  192.0.2.1             00-11-22-33-44-55     dynamic
  192.0.2.23            2c-cf-67-01-02-03     dynamic
  192.0.2.255           ff-ff-ff-ff-ff-ff     static
  224.0.0.22            01-00-5e-00-00-16     static
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
                    mac: [0x2C, 0xCF, 0x67, 1, 2, 3]
                },
            ]
        );
    }

    #[test]
    fn empty_output_gives_nothing() {
        assert!(parse("").is_empty());
    }
}
