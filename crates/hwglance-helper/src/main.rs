//! `hwglance-helper`: the small root-owned part of hwglance.
//!
//! It does two things and nothing else:
//!   1. at start, reads the SMBIOS tables (root-only) and writes `memory.json`;
//!   2. every second, turns the RAPL energy counters (root-only) into watts and writes `cpu_power.json`.
//!
//! Only one-second averages are published: finer power readings are what the PLATYPUS
//! side channel needs. No network, no other writes, no child processes.

use hwglance_proto::{HELPER_DIR, HelperDimm, HelperMemory, HelperPower};
use serde::Serialize;
use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const DMI_TABLE: &str = "/sys/firmware/dmi/tables/DMI";
const POWERCAP: &str = "/sys/class/powercap";
const INTERVAL: Duration = Duration::from_millis(1000);

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// SMBIOS type 17 (Memory Device)
// ---------------------------------------------------------------------------

fn u16_at(b: &[u8], off: usize) -> Option<u16> {
    Some(u16::from_le_bytes([*b.get(off)?, *b.get(off + 1)?]))
}

fn u32_at(b: &[u8], off: usize) -> Option<u32> {
    Some(u32::from_le_bytes([
        *b.get(off)?,
        *b.get(off + 1)?,
        *b.get(off + 2)?,
        *b.get(off + 3)?,
    ]))
}

/// SMBIOS strings are 1-based; index 0 means "none".
fn string_at(strings: &[&[u8]], index: Option<u8>) -> String {
    match index {
        Some(i) if i > 0 => strings
            .get(i as usize - 1)
            .map(|s| String::from_utf8_lossy(s).trim().to_string())
            .unwrap_or_default(),
        _ => String::new(),
    }
}

fn memory_type_name(t: u8) -> String {
    match t {
        0x18 => "DDR3".into(),
        0x1A => "DDR4".into(),
        0x1E => "LPDDR4".into(),
        0x22 => "DDR5".into(),
        0x23 => "LPDDR5".into(),
        other => format!("type 0x{other:02X}"),
    }
}

/// Speeds of 0 (unknown) and 0xFFFF (see the extended field) are reported as absent.
fn speed(v: Option<u16>) -> Option<u32> {
    v.filter(|s| *s != 0 && *s != 0xFFFF).map(u32::from)
}

/// One type 17 structure: `data` is the formatted area, `strings` the text that follows it.
fn parse_memory_device(data: &[u8], strings: &[&[u8]]) -> Option<HelperDimm> {
    let size_mb = match u16_at(data, 0x0C)? {
        0 | 0xFFFF => return None, // empty slot or unknown
        0x7FFF => u64::from(u32_at(data, 0x1C)? & 0x7FFF_FFFF),
        s if s & 0x8000 != 0 => u64::from(s & 0x7FFF) / 1024, // kilobytes
        s => u64::from(s),
    };
    let s = |off: usize| string_at(strings, data.get(off).copied());
    Some(HelperDimm {
        locator: s(0x10),
        bank: s(0x11),
        size_mb,
        mem_type: memory_type_name(*data.get(0x12)?),
        rated_mts: speed(u16_at(data, 0x15)),
        configured_mts: speed(u16_at(data, 0x20)),
        configured_mv: if data.len() >= 0x28 {
            u16_at(data, 0x26).filter(|v| *v != 0).map(u32::from)
        } else {
            None
        },
        manufacturer: s(0x17),
        part: s(0x1A),
    })
}

/// Walk the raw SMBIOS structure table and collect the populated memory devices.
fn parse_dmi(table: &[u8]) -> Vec<HelperDimm> {
    let mut dimms = Vec::new();
    let mut pos = 0;
    while pos + 4 <= table.len() {
        let kind = table[pos];
        let len = table[pos + 1] as usize;
        if kind == 127 || len < 4 || pos + len > table.len() {
            break; // end-of-table marker or damaged table
        }
        let data = &table[pos..pos + len];
        // Strings: NUL-terminated, the whole set ends with an extra NUL.
        let mut end = pos + len;
        while end + 1 < table.len() && !(table[end] == 0 && table[end + 1] == 0) {
            end += 1;
        }
        let text = &table[(pos + len).min(end)..end];
        let strings: Vec<&[u8]> = text.split(|b| *b == 0).filter(|s| !s.is_empty()).collect();
        if kind == 17
            && let Some(d) = parse_memory_device(data, &strings)
        {
            dimms.push(d);
        }
        pos = end + 2;
    }
    dimms
}

fn read_memory() -> io::Result<HelperMemory> {
    let table = fs::read(DMI_TABLE)?;
    Ok(HelperMemory {
        unix_ms: now_ms(),
        dimms: parse_dmi(&table),
    })
}

// ---------------------------------------------------------------------------
// RAPL energy counters
// ---------------------------------------------------------------------------

/// Energy used between two counter readings; the counter wraps at `max`.
fn delta_uj(prev: u64, now: u64, max: u64) -> u64 {
    if now >= prev {
        now - prev
    } else {
        now + max.saturating_sub(prev)
    }
}

struct Zone {
    energy: PathBuf,
    max_range_uj: u64,
    prev_uj: Option<u64>,
}

impl Zone {
    fn open(dir: &Path) -> Option<Zone> {
        let energy = dir.join("energy_uj");
        fs::read_to_string(&energy).ok()?; // not readable: no such zone for us
        let max = fs::read_to_string(dir.join("max_energy_range_uj"))
            .ok()?
            .trim()
            .parse()
            .ok()?;
        Some(Zone {
            energy,
            max_range_uj: max,
            prev_uj: None,
        })
    }

    /// Average power since the previous call, from the wrapping counter.
    fn watts(&mut self, elapsed: Duration) -> Option<f64> {
        let now: u64 = fs::read_to_string(&self.energy).ok()?.trim().parse().ok()?;
        let prev = self.prev_uj.replace(now)?;
        let delta = delta_uj(prev, now, self.max_range_uj);
        let secs = elapsed.as_secs_f64();
        (secs > 0.0).then(|| (delta as f64 / 1e6 / secs * 100.0).round() / 100.0)
    }
}

fn zone_name(dir: &Path) -> Option<String> {
    Some(
        fs::read_to_string(dir.join("name"))
            .ok()?
            .trim()
            .to_string(),
    )
}

/// The `package-0` zone and its `core` child.
fn open_zones() -> (Option<Zone>, Option<Zone>) {
    let pkg_dir = Path::new(POWERCAP).join("intel-rapl:0");
    let package = (zone_name(&pkg_dir).as_deref() == Some("package-0"))
        .then(|| Zone::open(&pkg_dir))
        .flatten();
    let core = fs::read_dir(POWERCAP)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("intel-rapl:0:"))
        .find(|e| zone_name(&e.path()).as_deref() == Some("core"))
        .and_then(|e| Zone::open(&e.path()));
    (package, core)
}

// ---------------------------------------------------------------------------
// Output
// ---------------------------------------------------------------------------

/// Write `value` as JSON to `HELPER_DIR/name`, world-readable, replacing the file atomically.
fn publish<T: Serialize>(name: &str, value: &T) -> io::Result<()> {
    let dir = Path::new(HELPER_DIR);
    let tmp = dir.join(format!(".{name}.tmp"));
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o644)
        .open(&tmp)?;
    f.write_all(&serde_json::to_vec(value).map_err(io::Error::other)?)?;
    f.sync_all().ok();
    drop(f);
    fs::rename(&tmp, dir.join(name))
}

fn main() {
    let mut once = false;
    for a in std::env::args().skip(1) {
        match a.as_str() {
            "--once" => once = true,
            "--version" | "-V" => {
                println!("hwglance-helper {}", env!("CARGO_PKG_VERSION"));
                return;
            }
            _ => {
                eprintln!("usage: hwglance-helper [--once | --version]");
                std::process::exit(2);
            }
        }
    }

    let memory = match read_memory() {
        Ok(m) => Some(m),
        Err(e) => {
            eprintln!("hwglance-helper: cannot read {DMI_TABLE}: {e}");
            None
        }
    };
    let (mut package, mut core) = open_zones();
    if package.is_none() {
        eprintln!("hwglance-helper: no readable RAPL package zone, no power readings");
    }

    if once {
        if let Some(m) = &memory {
            println!("{}", serde_json::to_string(m).unwrap_or_default());
        }
        let start = Instant::now();
        package
            .as_mut()
            .and_then(|z| z.watts(Duration::from_secs(1)));
        core.as_mut().and_then(|z| z.watts(Duration::from_secs(1)));
        thread::sleep(INTERVAL);
        let elapsed = start.elapsed();
        let p = HelperPower {
            unix_ms: now_ms(),
            package_w: package.as_mut().and_then(|z| z.watts(elapsed)),
            core_w: core.as_mut().and_then(|z| z.watts(elapsed)),
        };
        println!("{}", serde_json::to_string(&p).unwrap_or_default());
        return;
    }

    if let Some(m) = &memory
        && let Err(e) = publish("memory.json", m)
    {
        eprintln!("hwglance-helper: cannot write memory.json: {e}");
    }
    // Baseline reading, then one average per interval.
    package.as_mut().and_then(|z| z.watts(INTERVAL));
    core.as_mut().and_then(|z| z.watts(INTERVAL));
    let mut last = Instant::now();
    let mut next = last + INTERVAL;
    loop {
        thread::sleep(next.saturating_duration_since(Instant::now()));
        let now = Instant::now();
        let elapsed = now.duration_since(last);
        last = now;
        let p = HelperPower {
            unix_ms: now_ms(),
            package_w: package.as_mut().and_then(|z| z.watts(elapsed)),
            core_w: core.as_mut().and_then(|z| z.watts(elapsed)),
        };
        if let Err(e) = publish("cpu_power.json", &p) {
            eprintln!("hwglance-helper: cannot write cpu_power.json: {e}");
        }
        next += INTERVAL;
        if next < Instant::now() {
            next = Instant::now() + INTERVAL;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A type 17 structure as DDR5 DIMMs report it.
    fn dimm(size: u16, ext: u32) -> Vec<u8> {
        let mut d = vec![0u8; 0x28];
        d[0] = 17;
        d[1] = 0x28;
        d[0x0C..0x0E].copy_from_slice(&size.to_le_bytes());
        d[0x1C..0x20].copy_from_slice(&ext.to_le_bytes());
        d[0x10] = 1; // device locator
        d[0x11] = 2; // bank locator
        d[0x12] = 0x22; // DDR5
        d[0x15..0x17].copy_from_slice(&4800u16.to_le_bytes());
        d[0x17] = 3; // manufacturer
        d[0x1A] = 4; // part number
        d[0x20..0x22].copy_from_slice(&6000u16.to_le_bytes());
        d[0x26..0x28].copy_from_slice(&1100u16.to_le_bytes());
        d.extend_from_slice(b"DIMM 0\0P0 CHANNEL A\0G Skill Intl\0F5-6000J3038F16G\0\0");
        d
    }

    #[test]
    fn parses_populated_dimms_and_stops_at_end_marker() {
        let mut t = Vec::new();
        t.extend(dimm(0x4000, 0)); // 16 GB
        t.extend(dimm(0x7FFF, 32768)); // extended size
        t.extend(dimm(0, 0)); // empty slot
        t.extend([127, 4, 0, 0, 0, 0]);
        t.extend(dimm(0x4000, 0)); // after the end marker: ignored
        let d = parse_dmi(&t);
        assert_eq!(d.len(), 2);
        assert_eq!(d[0].size_mb, 16384);
        assert_eq!(d[1].size_mb, 32768);
        assert_eq!(d[0].mem_type, "DDR5");
        assert_eq!(
            (d[0].rated_mts, d[0].configured_mts, d[0].configured_mv),
            (Some(4800), Some(6000), Some(1100))
        );
        assert_eq!(
            (d[0].manufacturer.as_str(), d[0].part.as_str()),
            ("G Skill Intl", "F5-6000J3038F16G")
        );
        assert_eq!(
            (d[0].locator.as_str(), d[0].bank.as_str()),
            ("DIMM 0", "P0 CHANNEL A")
        );
    }

    #[test]
    fn damaged_table_does_not_panic() {
        assert!(parse_dmi(&[17, 0x28, 0, 0, 1, 2]).is_empty());
        assert!(parse_dmi(&[]).is_empty());
    }

    #[test]
    fn counter_wraparound() {
        assert_eq!(delta_uj(100, 350, 1000), 250);
        assert_eq!(delta_uj(900, 100, 1000), 200);
    }
}
