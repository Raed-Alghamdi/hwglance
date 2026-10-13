//! Names from the PCI ID database (`pci.ids`, installed by the `hwdata` or `pciutils-data`
//! packages). Read once at startup, never while sampling.

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

/// Where distributions put the database.
pub const PCI_IDS: &[&str] = &["/usr/share/hwdata/pci.ids", "/usr/share/misc/pci.ids"];

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PciNames {
    pub vendor: Option<String>,
    pub device: Option<String>,
    /// The board maker's name for this exact card (subsystem vendor and device).
    pub subsystem: Option<String>,
}

fn hex4(s: &str) -> Option<u16> {
    (s.len() == 4).then(|| u16::from_str_radix(s, 16).ok()).flatten()
}

/// Look `vendor:device` (and the subsystem, when given) up in a database in `pci.ids` format.
/// Streams the text and stops at the end of the vendor's block, so the cost is a part of one
/// read of the file.
pub fn lookup(mut r: impl BufRead, vendor: u16, device: u16, sub: Option<(u16, u16)>) -> PciNames {
    let mut out = PciNames::default();
    let (mut in_vendor, mut in_device) = (false, false);
    let mut line = String::new();
    loop {
        line.clear();
        if r.read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
        let l = line.trim_end();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        if let Some(rest) = l.strip_prefix("\t\t") {
            if in_device && let Some((s, name)) = rest.split_once("  ") {
                let mut ids = s.split(' ');
                let found = (ids.next().and_then(hex4), ids.next().and_then(hex4));
                if let Some((sv, sd)) = sub
                    && found == (Some(sv), Some(sd))
                {
                    out.subsystem = Some(name.trim().to_string());
                }
            }
        } else if let Some(rest) = l.strip_prefix('\t') {
            if in_device {
                break; // the next device: ours is done
            }
            if in_vendor && let Some((id, name)) = rest.split_once("  ") && hex4(id) == Some(device) {
                in_device = true;
                out.device = Some(name.trim().to_string());
            }
        } else if in_vendor {
            break; // the next vendor, or the class list
        } else if let Some((id, name)) = l.split_once("  ")
            && hex4(id) == Some(vendor)
        {
            in_vendor = true;
            out.vendor = Some(name.trim().to_string());
        }
    }
    out
}

/// [`lookup`] in the first database file that exists under `paths`.
pub fn lookup_in(paths: &[&str], vendor: u16, device: u16, sub: Option<(u16, u16)>) -> PciNames {
    paths
        .iter()
        .find_map(|p| File::open(Path::new(p)).ok())
        .map(|f| lookup(BufReader::new(f), vendor, device, sub))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    const DB: &str = "\
# comment
1002  Advanced Micro Devices, Inc. [AMD/ATI]
\t13c0  Granite Ridge [Radeon Graphics]
\t164e  Raphael
\t744c  Navi 31 [Radeon RX 7900 XT/7900 XTX/7900 GRE/7900M]
\t\t1002 0e3b  RX 7900 XTX / RX 7900 GRE [XFX]
\t\t1043 0506  TUF Gaming Radeon RX 7900 XTX OC
\t7480  Navi 33 [Radeon RX 7600/7600 XT/7600M XT/7600S/7700S / PRO W7600]
# a later vendor reusing an id
10de  NVIDIA Corporation
\t164e  Not this one
\t2684  AD102 [GeForce RTX 4090]
\t\t19da 3675  GeForce RTX 4090 GAMING OC

C 00  Unclassified device
\t00  Non-VGA unclassified device
";

    fn find(v: u16, d: u16, s: Option<(u16, u16)>) -> PciNames {
        lookup(DB.as_bytes(), v, d, s)
    }

    #[test]
    fn vendor_device_and_subsystem() {
        let n = find(0x1002, 0x164e, Some((0x1043, 0x8877)));
        assert_eq!(n.vendor.as_deref(), Some("Advanced Micro Devices, Inc. [AMD/ATI]"));
        assert_eq!(n.device.as_deref(), Some("Raphael"));
        assert_eq!(n.subsystem, None);
        let n = find(0x1002, 0x744c, Some((0x1043, 0x0506)));
        assert_eq!(n.subsystem.as_deref(), Some("TUF Gaming Radeon RX 7900 XTX OC"));
        let n = find(0x10de, 0x2684, Some((0x19da, 0x3675)));
        assert_eq!(n.device.as_deref(), Some("AD102 [GeForce RTX 4090]"));
        assert_eq!(n.subsystem.as_deref(), Some("GeForce RTX 4090 GAMING OC"));
    }

    #[test]
    fn unknown_ids_and_block_edges() {
        // a device id of another vendor is not found under this one
        assert_eq!(find(0x1002, 0x2684, None).device, None);
        // the vendor is known, the device is not
        let n = find(0x1002, 0x9999, None);
        assert!(n.vendor.is_some() && n.device.is_none());
        // unknown vendor
        assert_eq!(find(0xabcd, 0x0001, None), PciNames::default());
        // the last device of a vendor block (followed by the next vendor)
        assert_eq!(find(0x1002, 0x7480, None).device.as_deref().map(|d| &d[..8]), Some("Navi 33 "));
        // the class list is not mistaken for devices
        assert_eq!(find(0x0000, 0x0000, None), PciNames::default());
    }

    #[test]
    fn missing_database() {
        assert_eq!(lookup_in(&["/nonexistent/pci.ids"], 0x1002, 0x164e, None), PciNames::default());
    }
}
