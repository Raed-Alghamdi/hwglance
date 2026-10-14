//! Whole-disk throughput from /proc/diskstats, identity from sysfs, mountpoints from /proc/self/mounts.

use crate::util::read_trim;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

const RESCAN_EVERY: Duration = Duration::from_secs(10);

struct Disk {
    name: String,
    model: String,
    serial: String,
    /// NVMe controller (`nvme0`), used to find the drive's hwmon chip.
    controller: Option<String>,
    mounts: Vec<String>,
    prev: Option<Counters>,
}

#[derive(Clone, Copy)]
struct Counters {
    rd_sectors: u64,
    wr_sectors: u64,
    io_ticks: u64,
}

#[derive(Clone, Debug)]
pub struct DiskVal {
    pub serial: String,
    pub model: String,
    pub controller: Option<String>,
    /// Shortest first.
    pub mounts: Vec<String>,
    pub read_bps: Option<f64>,
    pub write_bps: Option<f64>,
    pub active_pct: Option<f64>,
}

pub struct DiskSampler {
    disks: Vec<Disk>,
    scanned: Option<Instant>,
}

fn is_whole_disk(name: &str) -> bool {
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if let Some(rest) = name.strip_prefix("nvme") {
        // nvme<ctrl>n<ns>, but not nvme0n1p1
        if let Some((c, n)) = rest.split_once('n') {
            return digits(c) && digits(n);
        }
        return false;
    }
    if let Some(rest) = name.strip_prefix("sd") {
        return !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_lowercase());
    }
    false
}

fn decode_mount_escapes(s: &str) -> String {
    // /proc/self/mounts writes space, tab, newline and backslash as \040 \011 \012 \134
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\'
            && i + 3 < b.len()
            && let Some(oct) = s.get(i + 1..i + 4)
            && let Ok(v) = u8::from_str_radix(oct, 8)
        {
            out.push(v);
            i += 4;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The whole disk a block device belongs to (`nvme2n1p2` -> `nvme2n1`), following device-mapper
/// stacks down to their first member.
fn parent_disk(dev: &str, depth: u32) -> Option<String> {
    let sys = fs::canonicalize(Path::new("/sys/class/block").join(dev)).ok()?;
    if sys.join("partition").exists() {
        return sys
            .parent()?
            .file_name()
            .map(|s| s.to_string_lossy().into_owned());
    }
    if depth < 3
        && let Some(first) = fs::read_dir(sys.join("slaves")).ok()?.flatten().next()
    {
        return parent_disk(&first.file_name().to_string_lossy(), depth + 1);
    }
    Some(dev.to_string())
}

/// disk name -> mountpoints, shortest first.
fn read_mounts() -> HashMap<String, Vec<String>> {
    let mut map: HashMap<String, Vec<String>> = HashMap::new();
    let text = fs::read_to_string("/proc/self/mounts").unwrap_or_default();
    let mut parents: HashMap<String, Option<String>> = HashMap::new();
    for line in text.lines() {
        let mut it = line.split_whitespace();
        let (Some(src), Some(mp)) = (it.next(), it.next()) else {
            continue;
        };
        let Some(dev) = src.strip_prefix("/dev/") else {
            continue;
        };
        let dev = match fs::canonicalize(src) {
            Ok(p) => p
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| dev.to_string()),
            Err(_) => dev.to_string(),
        };
        let disk = parents
            .entry(dev.clone())
            .or_insert_with(|| parent_disk(&dev, 0))
            .clone();
        if let Some(disk) = disk {
            let mp = decode_mount_escapes(mp);
            let list = map.entry(disk).or_default();
            if !list.contains(&mp) {
                list.push(mp);
            }
        }
    }
    for list in map.values_mut() {
        list.sort_by(|a, b| a.len().cmp(&b.len()).then_with(|| a.cmp(b)));
    }
    map
}

/// The datasheet operating temperature (upper end, degrees C) of an NVMe drive, from its model
/// name. Unlisted drives get `None` and the generic limits. Each entry is a model-name prefix:
/// - "Samsung SSD 990 PRO" 70: Samsung 990 PRO datasheet, 0 to 70 C measured by S.M.A.R.T.
///   temperature,
///   https://download.semiconductor.samsung.com/resources/data-sheet/Samsung_NVMe_SSD_990_PRO_Datasheet_Rev.1.0_10129514072296.pdf
/// - "T-FORCE TM8FP5" 70: the Phison E12 family (firmware ECFM12), Phison E12S brochure, 0 to 70 C,
///   https://www.phison.com/images/products_datasheet/ProductBrochure_Consumer_PS5012-E12S-032224.pdf
pub fn nvme_rated_max_c(model: &str) -> Option<f64> {
    const RATED: &[(&str, f64)] = &[("Samsung SSD 990 PRO", 70.0), ("T-FORCE TM8FP5", 70.0)];
    let model = model.trim_start();
    RATED
        .iter()
        .find(|(prefix, _)| model.get(..prefix.len()).is_some_and(|m| m.eq_ignore_ascii_case(prefix)))
        .map(|(_, c)| *c)
}

fn identify(name: &str) -> (String, String, Option<String>) {
    let dev = Path::new("/sys/block").join(name).join("device");
    let controller = fs::canonicalize(&dev)
        .ok()
        .and_then(|p| p.file_name().map(|s| s.to_string_lossy().into_owned()))
        .filter(|n| n.starts_with("nvme"));
    let model = read_trim(dev.join("model")).unwrap_or_else(|| name.to_string());
    let serial = read_trim(dev.join("serial"))
        .or_else(|| read_trim(Path::new("/sys/block").join(name).join("device/wwid")))
        .unwrap_or_else(|| name.to_string());
    (model, serial, controller)
}

impl DiskSampler {
    pub fn new() -> Self {
        DiskSampler {
            disks: Vec::new(),
            scanned: None,
        }
    }

    fn rescan(&mut self, names: &[String]) {
        let mounts = read_mounts();
        let mut disks = Vec::new();
        for name in names {
            let (model, serial, controller) = identify(name);
            let old = self.disks.iter().find(|d| &d.name == name);
            disks.push(Disk {
                name: name.clone(),
                model,
                serial,
                controller,
                mounts: mounts.get(name).cloned().unwrap_or_default(),
                prev: old.and_then(|d| d.prev),
            });
        }
        self.disks = disks;
        self.scanned = Some(Instant::now());
    }

    /// `dt` is the time since the previous call.
    pub fn sample(&mut self, dt: Option<Duration>) -> Vec<DiskVal> {
        let text = fs::read_to_string("/proc/diskstats").unwrap_or_default();
        let mut stats: HashMap<String, Counters> = HashMap::new();
        for line in text.lines() {
            let f: Vec<&str> = line.split_whitespace().collect();
            if f.len() < 13 || !is_whole_disk(f[2]) {
                continue;
            }
            let n = |i: usize| f[i].parse::<u64>().unwrap_or(0);
            stats.insert(
                f[2].to_string(),
                Counters {
                    rd_sectors: n(5),
                    wr_sectors: n(9),
                    io_ticks: n(12),
                },
            );
        }
        let mut names: Vec<String> = stats.keys().cloned().collect();
        names.sort();
        let known: Vec<&String> = self.disks.iter().map(|d| &d.name).collect();
        let changed = known.len() != names.len() || known.iter().zip(&names).any(|(a, b)| *a != b);
        if changed || self.scanned.is_none_or(|t| t.elapsed() >= RESCAN_EVERY) {
            self.rescan(&names);
        }
        let secs = dt.map(|d| d.as_secs_f64()).filter(|s| *s > 0.0);
        let mut out = Vec::new();
        for d in &mut self.disks {
            let Some(now) = stats.get(&d.name).copied() else {
                continue;
            };
            let (mut rd, mut wr, mut act) = (None, None, None);
            if let (Some(p), Some(secs)) = (d.prev, secs) {
                rd = Some(now.rd_sectors.saturating_sub(p.rd_sectors) as f64 * 512.0 / secs);
                wr = Some(now.wr_sectors.saturating_sub(p.wr_sectors) as f64 * 512.0 / secs);
                act = Some(
                    (now.io_ticks.saturating_sub(p.io_ticks) as f64 / (secs * 10.0))
                        .clamp(0.0, 100.0),
                );
            }
            d.prev = Some(now);
            out.push(DiskVal {
                serial: d.serial.clone(),
                model: d.model.clone(),
                controller: d.controller.clone(),
                mounts: d.mounts.clone(),
                read_bps: rd,
                write_bps: wr,
                active_pct: act,
            });
        }
        // The disk holding / first, then by mountpoint (unmounted disks last).
        out.sort_by(|a, b| {
            let key = |d: &DiskVal| {
                (
                    !d.mounts.iter().any(|m| m == "/"),
                    d.mounts.is_empty(),
                    d.mounts.first().cloned().unwrap_or_default(),
                    d.serial.clone(),
                )
            };
            key(a).cmp(&key(b))
        });
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whole_disks() {
        assert!(is_whole_disk("nvme0n1"));
        assert!(is_whole_disk("sda"));
        assert!(!is_whole_disk("nvme0n1p1"));
        assert!(!is_whole_disk("sda1"));
        assert!(!is_whole_disk("zram0"));
        assert!(!is_whole_disk("loop0"));
        assert!(!is_whole_disk("dm-0"));
    }

    #[test]
    fn nvme_ratings_match_model_name_prefixes() {
        assert_eq!(nvme_rated_max_c("Samsung SSD 990 PRO 2TB"), Some(70.0));
        assert_eq!(nvme_rated_max_c("samsung ssd 990 pro 1tb"), Some(70.0));
        assert_eq!(nvme_rated_max_c("T-FORCE TM8FP5001T"), Some(70.0));
        assert_eq!(nvme_rated_max_c("Samsung SSD 980 PRO 1TB"), None);
        assert_eq!(nvme_rated_max_c("T-FORCE TM8FP6001T"), None);
        assert_eq!(nvme_rated_max_c("nvme0n1"), None);
        assert_eq!(nvme_rated_max_c(""), None);
    }

    #[test]
    fn mount_escapes() {
        assert_eq!(decode_mount_escapes(r"/run/media/a\040b"), "/run/media/a b");
        assert_eq!(decode_mount_escapes("/plain"), "/plain");
    }
}
