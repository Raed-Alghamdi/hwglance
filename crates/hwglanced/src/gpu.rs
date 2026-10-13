//! The AMD and Intel graphics devices the kernel drives: found once at startup (name from the
//! driver or the PCI ID database, integrated or discrete) and sampled from sysfs. NVIDIA cards
//! go through NVML instead (nvml.rs). Read only: nothing here ever writes to sysfs.

use crate::pci::{self, PciNames};
use crate::util::{read_num, read_trim};
use std::fs;
use std::path::{Path, PathBuf};

pub const AMD: u16 = 0x1002;
pub const INTEL: u16 = 0x8086;
pub const NVIDIA: u16 = 0x10de;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Integrated,
    Discrete,
}

/// Which readings a card has at all (so a card that is asleep keeps its rows).
#[derive(Clone, Copy, Debug, Default)]
pub struct Caps {
    pub busy: bool,
    pub mem_busy: bool,
    pub vram: bool,
    pub power: bool,
    pub power_cap: bool,
    pub fan_rpm: bool,
    pub fan_pct: bool,
    pub clock_core: bool,
    pub clock_mem: bool,
    pub voltage: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Slot {
    /// `edge` (AMD), `pkg` (Intel xe): the main temperature.
    Core,
    /// `junction`: the hottest point on the die.
    Hotspot,
    /// `mem` (AMD), `vram` (Intel xe).
    Vram,
}

#[derive(Clone, Debug)]
struct TempSrc {
    slot: Slot,
    input: PathBuf,
    crit: Option<f64>,
}

#[derive(Clone, Debug)]
pub struct GpuInfo {
    /// Card id in the sensor table: `igpu` for the integrated GPU, `gpu1`, `gpu2`, ... for
    /// discrete ones in PCI address order (`gpu0` is the NVIDIA card NVML reports).
    pub card: String,
    pub name: String,
    pub kind: Kind,
    /// PCI address, `0000:0e:00.0`.
    pub pci: String,
    caps: Caps,
    paths: Paths,
}

#[derive(Clone, Debug, Default)]
struct Paths {
    runtime_status: PathBuf,
    temps: Vec<TempSrc>,
    busy: Option<PathBuf>,
    mem_busy: Option<PathBuf>,
    vram_used: Option<PathBuf>,
    vram_total: Option<f64>,
    power: Option<PathBuf>,
    power_cap: Option<PathBuf>,
    fan_rpm: Option<PathBuf>,
    pwm: Option<PathBuf>,
    pwm_max: f64,
    clock_core: Option<(PathBuf, f64)>,
    clock_mem: Option<(PathBuf, f64)>,
    voltage: Option<PathBuf>,
}

pub struct Found {
    pub gpus: Vec<GpuInfo>,
    /// An NVIDIA GPU is on the PCI bus (NVML is worth asking). True when the bus cannot be read.
    pub nvidia: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct GpuTemp {
    pub value: Option<f64>,
    pub crit: Option<f64>,
}

/// One sample of one card.
#[derive(Clone, Debug)]
pub struct CardVal {
    pub card: String,
    pub name: String,
    pub kind: Kind,
    pub caps: Caps,
    /// In runtime suspend: the card is not read, so sampling never wakes it.
    pub asleep: bool,
    /// `None` when the card has no such sensor.
    pub edge: Option<GpuTemp>,
    pub junction: Option<GpuTemp>,
    pub mem: Option<GpuTemp>,
    pub busy_pct: Option<f64>,
    pub mem_busy_pct: Option<f64>,
    pub vram_used: Option<f64>,
    pub vram_total: Option<f64>,
    pub power_w: Option<f64>,
    pub power_cap_w: Option<f64>,
    pub fan_rpm: Option<f64>,
    pub fan_pct: Option<f64>,
    pub clock_core_mhz: Option<f64>,
    pub clock_mem_mhz: Option<f64>,
    pub voltage_v: Option<f64>,
}

// ---------------------------------------------------------------------------
// Naming
// ---------------------------------------------------------------------------

fn vendor_short(v: u16) -> &'static str {
    match v {
        AMD => "AMD",
        INTEL => "Intel",
        NVIDIA => "NVIDIA",
        _ => "",
    }
}

/// Readable name of a card: the driver's product name, else the board maker's Radeon name,
/// else the PCI database's, else "AMD GPU (1002:164e)".
/// "Raphael" -> "AMD Radeon Graphics (Raphael)"; "Navi 31 [Radeon RX 7900 XTX]" -> "AMD Radeon RX 7900 XTX".
pub fn label(vendor: u16, device: u16, product: Option<&str>, names: &PciNames) -> String {
    if let Some(p) = product.map(str::trim).filter(|p| !p.is_empty()) {
        return p.to_string();
    }
    let brand = vendor_short(vendor);
    if vendor == AMD
        && let Some(sub) = names.subsystem.as_deref()
        && !sub.contains('/')
        && let Some(at) = sub.find("Radeon")
    {
        return format!("AMD {}", &sub[at..]);
    }
    let Some(dev) = names.device.as_deref() else {
        let brand = if brand.is_empty() { "PCI" } else { brand };
        return format!("{brand} GPU ({vendor:04x}:{device:04x})");
    };
    let inside = dev
        .rfind('[')
        .and_then(|a| Some(&dev[a + 1..dev.rfind(']')?]))
        .map(str::trim)
        .filter(|s| !s.is_empty());
    match (vendor, inside) {
        (AMD, Some(i)) if i.contains("Radeon") => format!("AMD {i}"),
        (INTEL, Some(i)) if i.starts_with("Intel") => i.to_string(),
        (INTEL, Some(i)) => format!("Intel {i}"),
        (AMD, _) => format!("AMD Radeon Graphics ({dev})"),
        (INTEL, None) => format!("Intel Graphics ({dev})"),
        _ => format!("{brand} {dev}").trim().to_string(),
    }
}

// ---------------------------------------------------------------------------
// Integrated or discrete
// ---------------------------------------------------------------------------

/// What an amdgpu device exposes, which tells a card from an APU's graphics.
#[derive(Clone, Copy, Debug, Default)]
pub struct Evidence {
    /// `junction` or `mem` temperature: only discrete chips have them (amdgpu hwmon docs: an
    /// APU has the edge temperature alone).
    pub junction_or_mem_temp: bool,
    /// A fan (fan1_input, pwm1): an APU has none, the CPU cooler cools it.
    pub fan: bool,
    pub power_cap: bool,
    /// Dedicated VRAM of 3 GiB or more. Weak on its own: an APU's carve-out can be set that large.
    pub big_vram: bool,
}

pub fn classify_amd(e: &Evidence) -> Kind {
    let score = 3 * e.junction_or_mem_temp as u32
        + 3 * e.fan as u32
        + e.power_cap as u32
        + e.big_vram as u32;
    if score >= 3 { Kind::Discrete } else { Kind::Integrated }
}

/// Intel's integrated graphics always sit at 00:02.0 on the first PCI bus; a card is anywhere else.
pub fn classify_intel(pci: &str) -> Kind {
    if pci.starts_with("0000:00:") { Kind::Integrated } else { Kind::Discrete }
}

// ---------------------------------------------------------------------------
// Discovery
// ---------------------------------------------------------------------------

fn read_hex(p: impl AsRef<Path>) -> Option<u32> {
    let s = read_trim(p)?;
    u32::from_str_radix(s.trim_start_matches("0x"), 16).ok()
}

fn first_file(dir: &Path, names: &[&str]) -> Option<PathBuf> {
    names.iter().map(|n| dir.join(n)).find(|p| p.exists())
}

fn temp_slot(label: &str, first: bool) -> Option<Slot> {
    match label {
        "edge" | "pkg" => Some(Slot::Core),
        "junction" | "hotspot" => Some(Slot::Hotspot),
        "mem" | "vram" => Some(Slot::Vram),
        // a driver that does not label its only sensor
        l if first && l.starts_with("temp") => Some(Slot::Core),
        _ => None,
    }
}

/// Sources in the hwmon directory of a card.
fn scan_hwmon(hw: &Path, p: &mut Paths, caps: &mut Caps) {
    let mut idx: Vec<u32> = fs::read_dir(hw)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            n.strip_prefix("temp")?.strip_suffix("_input")?.parse().ok()
        })
        .collect();
    idx.sort();
    for (n, i) in idx.iter().enumerate() {
        let label = read_trim(hw.join(format!("temp{i}_label"))).unwrap_or_else(|| format!("temp{i}"));
        let Some(slot) = temp_slot(&label, n == 0) else { continue };
        if p.temps.iter().any(|t| t.slot == slot) {
            continue;
        }
        p.temps.push(TempSrc {
            slot,
            input: hw.join(format!("temp{i}_input")),
            crit: read_num(hw.join(format!("temp{i}_crit"))).map(|m| m / 1000.0),
        });
    }
    p.power = first_file(hw, &["power1_average", "power1_input"]);
    p.power_cap = first_file(hw, &["power1_cap"]);
    p.fan_rpm = first_file(hw, &["fan1_input"]);
    p.pwm = first_file(hw, &["pwm1"]);
    p.pwm_max = read_num(hw.join("pwm1_max")).filter(|m| *m > 0.0).unwrap_or(255.0);
    // freqN_label: sclk is the shader clock, mclk the memory clock; values are in Hz
    for n in 1..=3 {
        let input = hw.join(format!("freq{n}_input"));
        if !input.exists() {
            continue;
        }
        match read_trim(hw.join(format!("freq{n}_label"))).as_deref() {
            Some("sclk") => p.clock_core = Some((input, 1e6)),
            Some("mclk") => p.clock_mem = Some((input, 1e6)),
            _ => {}
        }
    }
    p.voltage = first_file(hw, &["in0_input"]);
    caps.power = p.power.is_some();
    caps.power_cap = p.power_cap.is_some();
    caps.fan_rpm = p.fan_rpm.is_some();
    caps.fan_pct = p.pwm.is_some();
    caps.clock_core = p.clock_core.is_some();
    caps.clock_mem = p.clock_mem.is_some();
    caps.voltage = p.voltage.is_some();
}

/// Where an Intel driver keeps the current GT clock (MHz), relative to the DRM card.
fn intel_clock(card: &Path, dev: &Path) -> Option<(PathBuf, f64)> {
    let cands = [
        card.join("gt/gt0/rps_cur_freq_mhz"),
        card.join("gt_cur_freq_mhz"),
        dev.join("tile0/gt0/freq0/cur_freq"),
    ];
    cands.into_iter().find(|p| p.exists()).map(|p| (p, 1.0))
}

/// The ids a PCI device carries.
struct Ids {
    vendor: u16,
    device: u16,
    sub: Option<(u16, u16)>,
}

fn describe(card_dir: &Path, dev: &Path, ids: Ids, db: &[&str]) -> Option<(GpuInfo, Evidence)> {
    let vendor = ids.vendor;
    let driver = fs::read_link(dev.join("driver")).ok()?.file_name()?.to_string_lossy().into_owned();
    let ok = matches!((vendor, driver.as_str()), (AMD, "amdgpu") | (INTEL, "i915" | "xe"));
    if !ok {
        return None;
    }
    let pci = dev.file_name()?.to_string_lossy().into_owned();
    let sub = ids.sub.filter(|s| *s != (0, 0));
    let names = pci::lookup_in(db, vendor, ids.device, sub);
    let product = read_trim(dev.join("product_name"));
    let name = label(vendor, ids.device, product.as_deref(), &names);

    let mut paths = Paths { runtime_status: dev.join("power/runtime_status"), pwm_max: 255.0, ..Default::default() };
    let mut caps = Caps::default();
    if let Some(hw) = fs::read_dir(dev.join("hwmon")).ok().and_then(|d| {
        let mut v: Vec<PathBuf> = d.flatten().map(|e| e.path()).collect();
        v.sort();
        v.into_iter().next()
    }) {
        scan_hwmon(&hw, &mut paths, &mut caps);
    }
    if vendor == AMD {
        paths.busy = first_file(dev, &["gpu_busy_percent"]);
        paths.mem_busy = first_file(dev, &["mem_busy_percent"]);
        paths.vram_used = first_file(dev, &["mem_info_vram_used"]);
        paths.vram_total = read_num(dev.join("mem_info_vram_total"));
        caps.busy = paths.busy.is_some();
        caps.mem_busy = paths.mem_busy.is_some();
        caps.vram = paths.vram_used.is_some() && paths.vram_total.is_some();
    } else {
        let c = intel_clock(card_dir, dev);
        caps.clock_core = c.is_some();
        paths.clock_core = c;
    }
    let evidence = Evidence {
        junction_or_mem_temp: paths.temps.iter().any(|t| matches!(t.slot, Slot::Hotspot | Slot::Vram)),
        fan: caps.fan_rpm || caps.fan_pct,
        power_cap: caps.power_cap,
        big_vram: paths.vram_total.is_some_and(|b| b >= 3.0 * 1024.0 * 1024.0 * 1024.0),
    };
    let kind = if vendor == AMD { classify_amd(&evidence) } else { classify_intel(&pci) };
    let info = GpuInfo { card: String::new(), name, kind, pci, caps, paths };
    Some((info, evidence))
}

/// Whether the PCI bus holds an NVIDIA display device, or `None` when it cannot be listed.
fn nvidia_on_bus(sys: &Path) -> Option<bool> {
    let dir = fs::read_dir(sys.join("bus/pci/devices")).ok()?;
    Some(dir.flatten().any(|e| {
        let p = e.path();
        read_hex(p.join("vendor")) == Some(NVIDIA as u32)
            && read_hex(p.join("class")).is_some_and(|c| c >> 16 == 0x03)
    }))
}

/// The AMD and Intel GPUs under `sys` (normally `/sys`), with their card ids assigned.
pub fn discover_in(sys: &Path, pci_ids: &[&str]) -> Found {
    let mut nvidia = nvidia_on_bus(sys).unwrap_or(true);
    let mut seen: Vec<PathBuf> = Vec::new();
    let mut found: Vec<GpuInfo> = Vec::new();
    let mut cards: Vec<PathBuf> = fs::read_dir(sys.join("class/drm"))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .strip_prefix("card")
                .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        })
        .map(|e| e.path())
        .collect();
    cards.sort();
    for card_dir in cards {
        let dev = fs::canonicalize(card_dir.join("device")).unwrap_or_else(|_| card_dir.join("device"));
        if seen.contains(&dev) {
            continue;
        }
        seen.push(dev.clone());
        let (Some(v), Some(d)) = (read_hex(dev.join("vendor")), read_hex(dev.join("device"))) else {
            continue;
        };
        if v == NVIDIA as u32 {
            nvidia = true;
            continue;
        }
        let sub = match (read_hex(dev.join("subsystem_vendor")), read_hex(dev.join("subsystem_device"))) {
            (Some(a), Some(b)) => Some((a as u16, b as u16)),
            _ => None,
        };
        let ids = Ids { vendor: v as u16, device: d as u16, sub };
        if let Some((info, _)) = describe(&card_dir, &dev, ids, pci_ids) {
            found.push(info);
        }
    }
    found.sort_by(|a, b| a.pci.cmp(&b.pci));
    let mut gpus = Vec::new();
    let mut have_igpu = false;
    let mut next = 1;
    for mut g in found {
        match g.kind {
            // One integrated GPU gets a card; a second one (rare) is left out.
            Kind::Integrated if have_igpu => continue,
            Kind::Integrated => {
                have_igpu = true;
                g.card = "igpu".into();
            }
            Kind::Discrete => {
                g.card = format!("gpu{next}");
                next += 1;
            }
        }
        gpus.push(g);
    }
    Found { gpus, nvidia }
}

pub fn discover() -> Found {
    discover_in(Path::new("/sys"), pci::PCI_IDS)
}

// ---------------------------------------------------------------------------
// Sampling
// ---------------------------------------------------------------------------

pub struct GpuSampler {
    gpus: Vec<GpuInfo>,
}

impl GpuSampler {
    pub fn new(gpus: Vec<GpuInfo>) -> Self {
        GpuSampler { gpus }
    }

    pub fn sample(&self) -> Vec<CardVal> {
        self.gpus.iter().map(read_card).collect()
    }
}

fn read_card(g: &GpuInfo) -> CardVal {
    let p = &g.paths;
    let mut v = CardVal {
        card: g.card.clone(),
        name: g.name.clone(),
        kind: g.kind,
        caps: g.caps,
        asleep: read_trim(&p.runtime_status).as_deref() == Some("suspended"),
        edge: None,
        junction: None,
        mem: None,
        busy_pct: None,
        mem_busy_pct: None,
        vram_used: None,
        vram_total: p.vram_total,
        power_w: None,
        power_cap_w: None,
        fan_rpm: None,
        fan_pct: None,
        clock_core_mhz: None,
        clock_mem_mhz: None,
        voltage_v: None,
    };
    for t in &p.temps {
        let slot = match t.slot {
            Slot::Core => &mut v.edge,
            Slot::Hotspot => &mut v.junction,
            Slot::Vram => &mut v.mem,
        };
        *slot = Some(GpuTemp { value: None, crit: t.crit });
    }
    if v.asleep {
        return v; // reading a suspended card would wake it
    }
    for t in &p.temps {
        let value = read_num(&t.input).map(|m| m / 1000.0);
        let slot = match t.slot {
            Slot::Core => v.edge.as_mut(),
            Slot::Hotspot => v.junction.as_mut(),
            Slot::Vram => v.mem.as_mut(),
        };
        if let Some(s) = slot {
            s.value = value;
        }
    }
    let num = |f: &Option<PathBuf>| f.as_deref().and_then(read_num);
    v.busy_pct = num(&p.busy);
    v.mem_busy_pct = num(&p.mem_busy);
    v.vram_used = num(&p.vram_used);
    v.power_w = num(&p.power).map(|uw| uw / 1e6);
    v.power_cap_w = num(&p.power_cap).map(|uw| uw / 1e6);
    v.fan_rpm = num(&p.fan_rpm);
    v.fan_pct = num(&p.pwm).map(|pwm| (100.0 * pwm / p.pwm_max).clamp(0.0, 100.0));
    v.clock_core_mhz = p.clock_core.as_ref().and_then(|(f, div)| read_num(f).map(|x| x / div));
    v.clock_mem_mhz = p.clock_mem.as_ref().and_then(|(f, div)| read_num(f).map(|x| x / div));
    v.voltage_v = num(&p.voltage).map(|mv| mv / 1000.0);
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn names(device: &str, sub: Option<&str>) -> PciNames {
        PciNames { vendor: None, device: Some(device.into()), subsystem: sub.map(Into::into) }
    }

    #[test]
    fn labels_come_from_the_best_source() {
        let raphael = names("Raphael", None);
        assert_eq!(label(AMD, 0x164e, None, &raphael), "AMD Radeon Graphics (Raphael)");
        // the driver's own product name wins
        assert_eq!(label(AMD, 0x164e, Some(" AMD Radeon PRO W7800 "), &raphael), "AMD Radeon PRO W7800");
        // the marketing name in brackets
        let navi = names("Navi 31 [Radeon RX 7900 XT/7900 XTX/7900 GRE/7900M]", None);
        assert_eq!(label(AMD, 0x744c, None, &navi), "AMD Radeon RX 7900 XT/7900 XTX/7900 GRE/7900M");
        assert_eq!(
            label(AMD, 0x13c0, None, &names("Granite Ridge [Radeon Graphics]", None)),
            "AMD Radeon Graphics"
        );
        // a board maker's Radeon name is more exact; a list of models or a non-Radeon name is not used
        let tuf = names("Navi 31 [Radeon RX 7900 XT/7900 XTX]", Some("TUF Gaming Radeon RX 7900 XTX OC"));
        assert_eq!(label(AMD, 0x744c, None, &tuf), "AMD Radeon RX 7900 XTX OC");
        let laptop = names("Picasso/Raven 2 [Radeon Vega Series]", Some("ThinkPad E595"));
        assert_eq!(label(AMD, 0x15d8, None, &laptop), "AMD Radeon Vega Series");
        // Intel
        assert_eq!(label(INTEL, 0x46a6, None, &names("Alder Lake-P GT2 [Iris Xe Graphics]", None)), "Intel Iris Xe Graphics");
        assert_eq!(label(INTEL, 0x56a0, None, &names("DG2 [Arc A770]", None)), "Intel Arc A770");
        assert_eq!(label(INTEL, 0x7d55, None, &names("Meteor Lake-P [Intel Arc Graphics]", None)), "Intel Arc Graphics");
        assert_eq!(label(INTEL, 0x1234, None, &names("Odd Lake", None)), "Intel Graphics (Odd Lake)");
        // not in the database
        assert_eq!(label(AMD, 0x164e, None, &PciNames::default()), "AMD GPU (1002:164e)");
        assert_eq!(label(INTEL, 0x0042, None, &PciNames::default()), "Intel GPU (8086:0042)");
    }

    #[test]
    fn amd_integrated_or_discrete() {
        // this PC's APU: edge temperature, no fan, no cap, a small carve-out
        assert_eq!(classify_amd(&Evidence::default()), Kind::Integrated);
        // power cap and a large carve-out alone are not enough (Strix Halo style APUs)
        assert_eq!(
            classify_amd(&Evidence { power_cap: true, big_vram: true, ..Default::default() }),
            Kind::Integrated
        );
        // a card: junction and memory temperatures
        assert_eq!(
            classify_amd(&Evidence { junction_or_mem_temp: true, ..Default::default() }),
            Kind::Discrete
        );
        // an older card with only an edge sensor, but a fan
        assert_eq!(classify_amd(&Evidence { fan: true, ..Default::default() }), Kind::Discrete);
        // Intel: the graphics at 00:02.0 are integrated, anything else is a card
        assert_eq!(classify_intel("0000:00:02.0"), Kind::Integrated);
        assert_eq!(classify_intel("0000:03:00.0"), Kind::Discrete);
    }

    /// One fake PCI GPU under `root`, with `files` (relative to its device directory).
    fn fake_gpu(root: &Path, card: &str, pci: &str, driver: &str, ids: [&str; 4], files: &[(&str, &str)]) {
        let dev = root.join("sys/devices/pci0000:00").join(pci);
        fs::create_dir_all(dev.join("hwmon/hwmon3")).unwrap();
        fs::create_dir_all(dev.join("power")).unwrap();
        for (n, v) in ["vendor", "device", "subsystem_vendor", "subsystem_device"].iter().zip(ids) {
            fs::write(dev.join(n), format!("{v}\n")).unwrap();
        }
        fs::write(dev.join("power/runtime_status"), "active\n").unwrap();
        for (f, c) in files {
            fs::write(dev.join(f), c).unwrap();
        }
        let drv = root.join("sys/bus/pci/drivers").join(driver);
        fs::create_dir_all(&drv).unwrap();
        symlink(&drv, dev.join("driver")).unwrap();
        let cardp = root.join("sys/class/drm").join(card);
        fs::create_dir_all(&cardp).unwrap();
        symlink(&dev, cardp.join("device")).unwrap();
        let busdev = root.join("sys/bus/pci/devices");
        fs::create_dir_all(&busdev).unwrap();
        symlink(&dev, busdev.join(pci)).unwrap();
        fs::write(dev.join("class"), "0x030000\n").unwrap();
    }

    fn tree(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("hwglance-gpu-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    const DB: &str = "1002  Advanced Micro Devices, Inc. [AMD/ATI]\n\t164e  Raphael\n\t744c  Navi 31 [Radeon RX 7900 XT/7900 XTX/7900 GRE/7900M]\n10de  NVIDIA Corporation\n\t2684  AD102 [GeForce RTX 4090]\n";

    #[test]
    fn finds_the_integrated_and_discrete_cards_and_numbers_them() {
        let root = tree("find");
        let dbfile = root.join("pci.ids");
        fs::write(&dbfile, DB).unwrap();
        // an APU (card0), a discrete Radeon (card1) and an NVIDIA card (card2)
        fake_gpu(&root, "card0", "0000:0e:00.0", "amdgpu", ["0x1002", "0x164e", "0x1043", "0x8877"], &[("gpu_busy_percent", "7\n"), ("mem_info_vram_total", "536870912\n"), ("mem_info_vram_used", "1000\n")]);
        let hw = root.join("sys/devices/pci0000:00/0000:0e:00.0/hwmon/hwmon3");
        fs::write(hw.join("temp1_input"), "41000\n").unwrap();
        fs::write(hw.join("temp1_label"), "edge\n").unwrap();
        fs::write(hw.join("power1_input"), "9000000\n").unwrap();
        fake_gpu(&root, "card1", "0000:03:00.0", "amdgpu", ["0x1002", "0x744c", "0x1002", "0x0e3b"], &[("gpu_busy_percent", "55\n"), ("mem_busy_percent", "12\n"), ("mem_info_vram_total", "25753026560\n"), ("mem_info_vram_used", "8589934592\n")]);
        let hw = root.join("sys/devices/pci0000:00/0000:03:00.0/hwmon/hwmon3");
        for (n, label, v) in [(1, "edge", 50000), (2, "junction", 62000), (3, "mem", 70000)] {
            fs::write(hw.join(format!("temp{n}_input")), format!("{v}\n")).unwrap();
            fs::write(hw.join(format!("temp{n}_label")), format!("{label}\n")).unwrap();
        }
        fs::write(hw.join("temp2_crit"), "110000\n").unwrap();
        fs::write(hw.join("power1_average"), "250000000\n").unwrap();
        fs::write(hw.join("power1_cap"), "327000000\n").unwrap();
        fs::write(hw.join("fan1_input"), "1450\n").unwrap();
        fs::write(hw.join("pwm1"), "102\n").unwrap();
        fs::write(hw.join("freq1_input"), "2400000000\n").unwrap();
        fs::write(hw.join("freq1_label"), "sclk\n").unwrap();
        fs::write(hw.join("freq2_input"), "1249000000\n").unwrap();
        fs::write(hw.join("freq2_label"), "mclk\n").unwrap();
        fs::write(hw.join("in0_input"), "1050\n").unwrap();
        fake_gpu(&root, "card2", "0000:01:00.0", "nvidia", ["0x10de", "0x2684", "0x19da", "0x3675"], &[]);
        // connector entries and an unsupported driver are skipped
        fs::create_dir_all(root.join("sys/class/drm/card1-DP-1")).unwrap();
        fake_gpu(&root, "card3", "0000:09:00.0", "ast", ["0x1a03", "0x2000", "0x0000", "0x0000"], &[]);

        let sys = root.join("sys");
        let f = discover_in(&sys, &[dbfile.to_str().unwrap()]);
        assert!(f.nvidia);
        let ids: Vec<(&str, &str, Kind)> = f.gpus.iter().map(|g| (g.card.as_str(), g.name.as_str(), g.kind)).collect();
        assert_eq!(
            ids,
            [
                ("gpu1", "AMD Radeon RX 7900 XT/7900 XTX/7900 GRE/7900M", Kind::Discrete),
                ("igpu", "AMD Radeon Graphics (Raphael)", Kind::Integrated),
            ]
        );

        let vals = GpuSampler::new(f.gpus).sample();
        let d = vals.iter().find(|v| v.card == "gpu1").unwrap();
        assert_eq!(d.edge.unwrap().value, Some(50.0));
        assert_eq!(d.junction.unwrap().crit, Some(110.0));
        assert_eq!(d.mem.unwrap().value, Some(70.0));
        assert_eq!((d.busy_pct, d.mem_busy_pct), (Some(55.0), Some(12.0)));
        assert_eq!((d.vram_used, d.vram_total), (Some(8589934592.0), Some(25753026560.0)));
        assert_eq!((d.power_w, d.power_cap_w), (Some(250.0), Some(327.0)));
        assert_eq!((d.fan_rpm, d.fan_pct), (Some(1450.0), Some(40.0)));
        assert_eq!((d.clock_core_mhz, d.clock_mem_mhz, d.voltage_v), (Some(2400.0), Some(1249.0), Some(1.05)));
        let i = vals.iter().find(|v| v.card == "igpu").unwrap();
        assert!(i.junction.is_none() && i.mem.is_none());
        assert_eq!((i.edge.unwrap().value, i.busy_pct, i.power_w), (Some(41.0), Some(7.0), Some(9.0)));

        // a card in runtime suspend is not read
        fs::write(root.join("sys/devices/pci0000:00/0000:03:00.0/power/runtime_status"), "suspended\n").unwrap();
        let asleep = GpuSampler::new(discover_in(&sys, &[dbfile.to_str().unwrap()]).gpus).sample();
        let d = asleep.iter().find(|v| v.card == "gpu1").unwrap();
        assert!(d.asleep && d.busy_pct.is_none() && d.edge.unwrap().value.is_none() && d.caps.busy);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn no_nvidia_and_no_database() {
        let root = tree("amdonly");
        fake_gpu(&root, "card0", "0000:0e:00.0", "amdgpu", ["0x1002", "0x164e", "0x1043", "0x8877"], &[]);
        let f = discover_in(&root.join("sys"), &["/nonexistent/pci.ids"]);
        assert!(!f.nvidia);
        assert_eq!(f.gpus[0].name, "AMD GPU (1002:164e)");
        // an unreadable bus: assume NVIDIA may be there, so NVML is tried as before
        assert!(discover_in(&root.join("nowhere"), &[]).nvidia);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn intel_graphics_have_a_name_and_a_clock() {
        let root = tree("intel");
        fake_gpu(&root, "card0", "0000:00:02.0", "i915", ["0x8086", "0x46a6", "0x1043", "0x1234"], &[]);
        let card = root.join("sys/class/drm/card0/gt/gt0");
        fs::create_dir_all(&card).unwrap();
        fs::write(card.join("rps_cur_freq_mhz"), "650\n").unwrap();
        let db = root.join("pci.ids");
        fs::write(&db, "8086  Intel Corporation\n\t46a6  Alder Lake-P GT2 [Iris Xe Graphics]\n").unwrap();
        let f = discover_in(&root.join("sys"), &[db.to_str().unwrap()]);
        assert_eq!((f.gpus[0].card.as_str(), f.gpus[0].name.as_str()), ("igpu", "Intel Iris Xe Graphics"));
        let v = &GpuSampler::new(f.gpus).sample()[0];
        assert_eq!(v.clock_core_mhz, Some(650.0));
        let _ = fs::remove_dir_all(&root);
    }
}
