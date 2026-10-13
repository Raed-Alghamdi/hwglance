//! Turns the raw samples into the sensor table (ids, labels, order, thresholds) and the summary.

use crate::config::Config;
use crate::cpu::{CpuVal, ThreadVal};
use crate::describe;
use crate::disks::DiskVal;
use crate::gpu::{CardVal, GpuTemp, Kind};
use crate::hwmon::{self, ChipVal, TempVal};
use crate::lact::LactVal;
use crate::memory::MemVal;
use crate::nvml::GpuVal;
use crate::util::slug;
use hwglance_proto::{HelperMemory, HelperPower, Reading, Section, SensorGroup, Summary, Unit};
use std::collections::{BTreeMap, HashMap};

const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

pub struct Inputs<'a> {
    pub cfg: &'a Config,
    pub cpu: &'a CpuVal,
    /// Fast and slow hwmon chips together.
    pub chips: &'a [ChipVal],
    pub mem: Option<&'a MemVal>,
    pub disks: &'a [DiskVal],
    pub gpu: Option<&'a GpuVal>,
    /// AMD and Intel graphics devices read from sysfs.
    pub gpus: &'a [CardVal],
    pub lact: Option<&'a LactVal>,
    /// Only when fresh.
    pub helper_power: Option<&'a HelperPower>,
    pub helper_mem: Option<&'a HelperMemory>,
    pub board_name: &'a str,
    /// Median of recent iGPU PPT readings (the package power fallback).
    pub ppt_estimate: Option<f64>,
}

fn rd(id: impl Into<String>, label: impl Into<String>, unit: Unit, value: Option<f64>) -> Reading {
    Reading {
        id: id.into(),
        label: label.into(),
        unit,
        value,
        stats: None,
        note: None,
        desc: None,
        events: None,
        warn: None,
        crit: None,
    }
}

trait ReadingExt {
    fn note(self, n: &str) -> Self;
    fn thr(self, warn: f64, crit: f64) -> Self;
}

impl ReadingExt for Reading {
    fn note(mut self, n: &str) -> Self {
        self.note = Some(n.to_string());
        self
    }
    fn thr(mut self, warn: f64, crit: f64) -> Self {
        self.warn = Some(warn);
        self.crit = Some(crit);
        self
    }
}

/// A yes/no reading. Flags that mean trouble carry `crit` 1 so they colour red while on.
fn flag(id: &str, label: &str, on: Option<bool>) -> Reading {
    rd(id, label, Unit::Flag, on.map(|b| if b { 1.0 } else { 0.0 }))
}

fn sec(id: &str, title: &str, collapsed: bool, readings: Vec<Reading>) -> Section {
    Section {
        id: id.into(),
        title: title.into(),
        collapsed_by_default: collapsed,
        readings,
        summary: None,
    }
}

fn chips_named<'a>(chips: &'a [ChipVal], name: &'a str) -> impl Iterator<Item = &'a ChipVal> {
    chips.iter().filter(move |c| c.name == name)
}

fn temp<'a>(chip: Option<&'a ChipVal>, label: &str) -> Option<&'a TempVal> {
    chip?.temps.iter().find(|t| t.label == label)
}

fn mean(v: impl Iterator<Item = f64>) -> Option<f64> {
    let (sum, n) = v.fold((0.0, 0usize), |(s, n), x| (s + x, n + 1));
    (n > 0).then(|| sum / n as f64)
}

fn max(v: impl Iterator<Item = f64>) -> Option<f64> {
    v.fold(None, |m: Option<f64>, x| Some(m.map_or(x, |m| m.max(x))))
}

/// "AMD Ryzen 7 7800X3D 8-Core Processor" -> "AMD Ryzen 7 7800X3D".
fn short_cpu_name(model: &str) -> String {
    let words: Vec<&str> = model.split_whitespace().collect();
    let n = words.len();
    if n >= 2
        && words[n - 1] == "Processor"
        && words[n - 2]
            .strip_suffix("-Core")
            .is_some_and(|c| c.parse::<u32>().is_ok())
    {
        return words[..n - 2].join(" ");
    }
    model.to_string()
}

/// Memory makers as they are printed on the stick.
fn short_vendor(vendor: &str) -> String {
    match vendor.trim() {
        "G Skill Intl" => "G.Skill".to_string(),
        v => v.to_string(),
    }
}

/// Short name of where a drive is mounted: `/` for the root filesystem, the folder name for
/// `/run/media/<user>/<name>`, else the last component of the first mount.
fn short_mount(mounts: &[String]) -> String {
    if mounts.iter().any(|m| m == "/") {
        return "/".into();
    }
    let first = mounts.first().map(String::as_str).unwrap_or("");
    first
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("/")
        .to_string()
}

/// `gpu0`, `gpu1`, ...: a discrete GPU card.
fn is_gpu_card(id: &str) -> bool {
    id.strip_prefix("gpu").is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// Section order in every card (temperatures first) and which sections start collapsed.
fn arrange(g: &mut SensorGroup) {
    const COLLAPSED: &[&str] = &[
        "cpu/usage",
        "cpu/clock",
        "cpu/thread",
        "gpu0/vramchips",
        "board/superio",
    ];
    let order: Vec<String> = match g.id.as_str() {
        "cpu" => ["temp", "limits", "power", "usage", "clock", "thread"]
            .iter()
            .map(|s| format!("cpu/{s}"))
            .collect(),
        id if is_gpu_card(id) => [
            "temp",
            "limits",
            "load",
            "power",
            "clock",
            "mem",
            "vramchips",
        ]
        .iter()
        .map(|s| format!("{id}/{s}"))
        .collect(),
        "memory" => ["temp", "usage", "speed"]
            .iter()
            .map(|s| format!("memory/{s}"))
            .collect(),
        "board" => ["temp", "fan", "superio"]
            .iter()
            .map(|s| format!("board/{s}"))
            .collect(),
        id if id.starts_with("disk/") => {
            ["temp", "io"].iter().map(|s| format!("{id}/{s}")).collect()
        }
        _ => Vec::new(),
    };
    if !order.is_empty() {
        g.sections
            .sort_by_key(|s| order.iter().position(|o| *o == s.id).unwrap_or(order.len()));
    }
    for s in &mut g.sections {
        if g.id != "igpu" {
            s.collapsed_by_default = COLLAPSED.contains(&s.id.as_str());
        }
    }
}

pub fn build(i: &Inputs) -> Vec<SensorGroup> {
    let mut groups = vec![cpu_group(i)];
    groups.extend(gpu_group(i));
    groups.extend(gpu_cards(i));
    groups.push(memory_group(i));
    groups.extend(disk_groups(i));
    groups.push(board_group(i));
    groups.extend(igpu_group(i));
    groups.retain(|g| !g.sections.is_empty());
    groups.iter_mut().for_each(arrange);
    let sensor = cpu_sensor(i);
    let ctx = describe::Ctx {
        threads: i.cpu.threads.len(),
        max_mhz: i.cpu.max_mhz,
        cpu_limit_c: cpu_limit(i, sensor),
        cpu_throttle_reported: i.cpu.throttling.is_some(),
    };
    describe::describe(&mut groups, &ctx);
    describe::add_summaries(&mut groups);
    groups
}

// ---------------------------------------------------------------------------
// CPU
// ---------------------------------------------------------------------------

/// The sensor that tells whether the CPU is at its limit: AMD's die temperature (Tdie where the
/// chip adds an offset to Tctl, else Tctl) or Intel's package temperature.
fn cpu_sensor<'a>(i: &Inputs<'a>) -> Option<&'a TempVal> {
    let k10 = chips_named(i.chips, "k10temp").next();
    temp(k10, "Tdie").or(temp(k10, "Tctl")).or_else(|| {
        chips_named(i.chips, "coretemp")
            .next()
            .and_then(|c| c.temps.iter().find(|t| t.label.starts_with("Package id")))
    })
}

/// Temperature at which the CPU lowers its clocks, in order of trust: the user's setting, the
/// limit the driver reports (`temp*_crit`, else `temp*_max`), AMD's published figure for the
/// model, else 95.
fn cpu_limit(i: &Inputs, sensor: Option<&TempVal>) -> f64 {
    let sane = |c: &f64| (40.0..=125.0).contains(c);
    i.cfg
        .cpu_thermal_limit_c
        .or(sensor.and_then(|t| t.crit.or(t.max)).filter(sane))
        .or(i.cpu.model_limit_c)
        .unwrap_or(95.0)
}

fn cpu_group(i: &Inputs) -> SensorGroup {
    let mut cores: BTreeMap<u32, Vec<&ThreadVal>> = BTreeMap::new();
    for t in &i.cpu.threads {
        cores.entry(t.core).or_default().push(t);
    }
    for v in cores.values_mut() {
        v.sort_by_key(|t| t.t);
    }

    let mut usage = vec![rd("cpu/usage/total", "Total", Unit::Percent, i.cpu.total)];
    let mut clock = vec![
        rd(
            "cpu/clock/avg",
            "Average",
            Unit::Mhz,
            mean(i.cpu.threads.iter().filter_map(|t| t.mhz)),
        ),
        rd(
            "cpu/clock/max",
            "Highest",
            Unit::Mhz,
            max(i.cpu.threads.iter().filter_map(|t| t.mhz)),
        ),
    ];
    for (core, th) in &cores {
        usage.push(rd(
            format!("cpu/usage/core{core}"),
            format!("Core {core}"),
            Unit::Percent,
            mean(th.iter().filter_map(|t| t.usage)),
        ));
        clock.push(rd(
            format!("cpu/clock/core{core}"),
            format!("Core {core}"),
            Unit::Mhz,
            max(th.iter().filter_map(|t| t.mhz)),
        ));
    }

    let mut per_thread = Vec::new();
    for (core, th) in &cores {
        for t in th {
            per_thread.push(rd(
                format!("cpu/thread/c{core}t{}/usage", t.t),
                format!("Core {core} T{} usage", t.t),
                Unit::Percent,
                t.usage,
            ));
        }
    }
    for (core, th) in &cores {
        for t in th {
            per_thread.push(rd(
                format!("cpu/thread/c{core}t{}/clock", t.t),
                format!("Core {core} T{} clock", t.t),
                Unit::Mhz,
                t.mhz,
            ));
        }
    }

    let sensor = cpu_sensor(i);
    let limit = cpu_limit(i, sensor);
    let warn = limit - 9.0;
    let mut temps = Vec::new();
    // AMD: k10temp
    let k10 = chips_named(i.chips, "k10temp").next();
    if let Some(c) = k10 {
        if let Some(t) = temp(k10, "Tctl") {
            temps.push(rd("cpu/temp/tctl", "Tctl", Unit::Celsius, t.value).thr(warn, limit));
        }
        let mut ccds: Vec<(u32, &TempVal)> = c
            .temps
            .iter()
            .filter_map(|t| Some((t.label.strip_prefix("Tccd")?.parse().ok()?, t)))
            .collect();
        ccds.sort_by_key(|(n, _)| *n);
        for (n, t) in ccds {
            temps.push(
                rd(
                    format!("cpu/temp/tccd{n}"),
                    format!("CCD {n}"),
                    Unit::Celsius,
                    t.value,
                )
                .thr(warn, limit),
            );
        }
    }
    // Intel: coretemp, one chip per package
    for (n, c) in chips_named(i.chips, "coretemp").enumerate() {
        let (pkg_id, pkg_label, prefix) = if n == 0 {
            ("cpu/temp/package".to_string(), "Package".to_string(), String::new())
        } else {
            (format!("cpu/temp/package{n}"), format!("Package {n}"), format!("p{n}"))
        };
        for t in c.temps.iter().filter(|t| t.label.starts_with("Package id")) {
            temps.push(rd(pkg_id.clone(), pkg_label.clone(), Unit::Celsius, t.value).thr(warn, limit));
        }
        let mut cores: Vec<(u32, &TempVal)> = c
            .temps
            .iter()
            .filter_map(|t| Some((t.label.strip_prefix("Core ")?.parse().ok()?, t)))
            .collect();
        cores.sort_by_key(|(n, _)| *n);
        for (core, t) in cores {
            temps.push(
                rd(
                    format!("cpu/temp/{prefix}core{core}"),
                    format!("Core {core}"),
                    Unit::Celsius,
                    t.value,
                )
                .thr(warn, limit),
            );
        }
    }
    let ec = chips_named(i.chips, "asusec").next();
    if let Some(t) = temp(ec, "CPU") {
        temps.push(rd("cpu/temp/board_cpu", "CPU (board)", Unit::Celsius, t.value).thr(warn, limit));
    }
    if let Some(t) = temp(ec, "CPU Package") {
        temps.push(
            rd(
                "cpu/temp/board_package",
                "CPU package (board)",
                Unit::Celsius,
                t.value,
            )
            .thr(warn, limit),
        );
    }

    // Intel reports throttling itself. AMD does not report it to Linux: infer it from the
    // CPU's temperature sitting at its limit.
    let (throttling, note) = match (i.cpu.throttling, sensor) {
        (Some(on), _) => (Some(on), "from the CPU's thermal throttle counters".to_string()),
        (None, Some(s)) => (
            s.value.map(|v| v >= limit - 0.5),
            format!("inferred from {}", s.label),
        ),
        (None, None) => (None, "no temperature or throttle counter available".to_string()),
    };
    let mut cpu_throttle = flag("cpu/throttle/thermal", "Thermal throttling", throttling).note(&note);
    cpu_throttle.crit = Some(1.0);

    let mut power = Vec::new();
    if let Some(h) = i.helper_power {
        power.push(
            rd("cpu/power/package", "Package", Unit::Watt, h.package_w)
                .note("RAPL energy counters read by hwglance-helper, 1 s average"),
        );
    } else if i.ppt_estimate.is_some() {
        power.push(
            rd("cpu/power/package", "Package", Unit::Watt, i.ppt_estimate).note(
                "rough estimate from the integrated GPU's PPT sensor; hwglance-helper gives exact values",
            ),
        );
    } else {
        power.push(
            rd("cpu/power/package", "Package", Unit::Watt, None).note("no power source available"),
        );
    }

    SensorGroup {
        id: "cpu".into(),
        title: "CPU".into(),
        subtitle: Some(short_cpu_name(&i.cpu.model)),
        sections: vec![
            sec("cpu/usage", "Usage", false, usage),
            sec("cpu/clock", "Clocks", false, clock),
            sec("cpu/thread", "Per thread", true, per_thread),
            sec("cpu/temp", "Temperatures", false, temps),
            sec("cpu/limits", "Limits", false, vec![cpu_throttle]),
            sec("cpu/power", "Power", false, power),
        ],
    }
}

// ---------------------------------------------------------------------------
// GPU
// ---------------------------------------------------------------------------

fn gpu_group(i: &Inputs) -> Option<SensorGroup> {
    let g = i.gpu?;
    let l = i.lact;
    let lact_note = |what: &str| format!("{what}, LACT (NVAPI)");

    let load = vec![
        rd("gpu0/load/core", "Core", Unit::Percent, g.core_pct),
        rd(
            "gpu0/load/memctl",
            "Memory controller",
            Unit::Percent,
            g.memctl_pct,
        ),
        rd("gpu0/load/enc", "Video encoder", Unit::Percent, g.enc_pct),
        rd("gpu0/load/dec", "Video decoder", Unit::Percent, g.dec_pct),
    ];
    let temps = vec![
        rd("gpu0/temp/core", "Core", Unit::Celsius, g.temp_c).thr(80.0, 88.0),
        rd(
            "gpu0/temp/hotspot",
            "Hotspot",
            Unit::Celsius,
            l.and_then(|l| l.hotspot_c),
        )
        .note("LACT (NVAPI)")
        .thr(95.0, 105.0),
        rd(
            "gpu0/temp/vram",
            "VRAM",
            Unit::Celsius,
            l.and_then(|l| l.vram_c),
        )
        .note(&lact_note("memory junction"))
        .thr(90.0, 100.0),
    ];
    let mut thermal = flag(
        "gpu0/throttle/thermal",
        "Thermal throttling",
        g.thermal_throttle,
    );
    thermal.crit = Some(1.0);
    let limits = vec![
        thermal,
        flag(
            "gpu0/throttle/power",
            "Power limit reached",
            g.power_throttle,
        ),
    ];
    let chips: Vec<Reading> = l
        .map(|l| {
            l.vram_chips
                .iter()
                .map(|(name, v)| {
                    rd(
                        format!("gpu0/vramchip/{name}"),
                        format!("Chip {name}"),
                        Unit::Celsius,
                        Some(*v),
                    )
                    .thr(90.0, 100.0)
                })
                .collect()
        })
        .unwrap_or_default();
    let clocks = vec![
        rd("gpu0/clock/core", "Core", Unit::Mhz, g.clock_core),
        rd("gpu0/clock/mem", "Memory", Unit::Mhz, g.clock_mem),
        rd("gpu0/clock/video", "Video", Unit::Mhz, g.clock_video),
    ];
    let pct = match (g.vram_used, g.vram_total) {
        (Some(u), Some(t)) if t > 0.0 => Some(100.0 * u / t),
        _ => None,
    };
    let mem = vec![
        rd("gpu0/mem/used", "VRAM used", Unit::Bytes, g.vram_used),
        rd("gpu0/mem/usage", "VRAM usage", Unit::Percent, pct),
    ];
    let power = vec![
        rd("gpu0/power/draw", "Power draw", Unit::Watt, g.power_w),
        rd(
            "gpu0/power/limit",
            "Power limit",
            Unit::Watt,
            g.power_limit_w,
        ),
        rd(
            "gpu0/power/voltage",
            "Core voltage",
            Unit::Volt,
            l.and_then(|l| l.voltage_v),
        )
        .note("LACT"),
        rd("gpu0/fan/pct", "Fan", Unit::Percent, g.fan_pct),
        rd(
            "gpu0/fan/rpm",
            "Fan speed",
            Unit::Rpm,
            l.and_then(|l| l.fan_rpm),
        )
        .note("LACT"),
    ];
    let subtitle = match g.vram_total {
        Some(t) => format!("{} · {:.0} GB", g.name, t / GIB),
        None => g.name.clone(),
    };
    Some(SensorGroup {
        id: "gpu0".into(),
        title: "GPU".into(),
        subtitle: Some(subtitle),
        sections: vec![
            sec("gpu0/load", "Usage", false, load),
            sec("gpu0/temp", "Temperatures", false, temps),
            sec("gpu0/limits", "Limits", false, limits),
            sec("gpu0/vramchips", "VRAM chips", true, chips),
            sec("gpu0/clock", "Clocks", false, clocks),
            sec("gpu0/mem", "Memory", false, mem),
            sec("gpu0/power", "Power and fan", false, power),
        ],
    })
}

// ---------------------------------------------------------------------------
// Memory
// ---------------------------------------------------------------------------

fn memory_group(i: &Inputs) -> SensorGroup {
    let helper = i.helper_mem;
    let configured = helper.and_then(|h| h.dimms.iter().filter_map(|d| d.configured_mts).max());

    let size_gb = match (helper, i.mem) {
        (Some(h), _) => Some(h.dimms.iter().map(|d| d.size_mb).sum::<u64>() as f64 / 1024.0),
        (None, Some(m)) => {
            // MemTotal is a few GB under the installed size (firmware and GPU reservations):
            // round up to a multiple of 4 GB.
            Some((m.total as f64 / GIB / 4.0).ceil() * 4.0)
        }
        _ => None,
    };
    let mut subtitle = size_gb.map(|g| format!("{g:.0} GB")).unwrap_or_default();
    if let Some(h) = helper
        && let Some(first) = h.dimms.first()
    {
        if let Some(mts) = configured {
            subtitle.push_str(&format!(" {}-{mts}", first.mem_type));
        }
        let part = format!("{} {}", short_vendor(&first.manufacturer), first.part)
            .trim()
            .to_string();
        subtitle.push_str(&format!(" · {} x {part}", h.dimms.len()));
    }

    let mut sections = Vec::new();
    if let Some(m) = i.mem {
        let used = m.total.saturating_sub(m.available) as f64;
        sections.push(sec(
            "memory/usage",
            "Usage",
            false,
            vec![
                rd("memory/usage/used", "Used", Unit::Bytes, Some(used)),
                rd(
                    "memory/usage/available",
                    "Available",
                    Unit::Bytes,
                    Some(m.available as f64),
                ),
                rd(
                    "memory/usage/pct",
                    "Usage",
                    Unit::Percent,
                    Some(100.0 * used / m.total as f64),
                )
                .thr(85.0, 95.0),
                rd(
                    "memory/usage/cached",
                    "Cache",
                    Unit::Bytes,
                    Some(m.cache as f64),
                ),
                rd(
                    "memory/usage/swap",
                    "Swap used",
                    Unit::Bytes,
                    Some(m.swap_used as f64),
                ),
            ],
        ));
    }
    let mut spd: Vec<&ChipVal> = chips_named(i.chips, "spd5118").collect();
    spd.sort_by(|a, b| a.dev.cmp(&b.dev));
    let dimm_temps: Vec<Reading> = spd
        .iter()
        .enumerate()
        .filter_map(|(n, c)| {
            let t = c.temps.first()?;
            Some(
                rd(
                    format!("memory/temp/dimm{}", n + 1),
                    format!("DIMM {}", n + 1),
                    Unit::Celsius,
                    t.value,
                )
                .note(&format!("spd5118 at {}", c.dev))
                .thr(60.0, 75.0),
            )
        })
        .collect();
    if !dimm_temps.is_empty() {
        sections.push(sec("memory/temp", "Temperatures", false, dimm_temps));
    }
    if let Some(mts) = configured {
        sections.push(sec(
            "memory/speed",
            "Speed",
            false,
            vec![
                rd(
                    "memory/speed/mts",
                    "Transfer rate",
                    Unit::Mts,
                    Some(mts as f64),
                )
                .note("configured speed from the SMBIOS tables, read by hwglance-helper"),
                rd(
                    "memory/speed/clock",
                    "Memory clock",
                    Unit::Mhz,
                    Some(mts as f64 / 2.0),
                ),
            ],
        ));
    }
    SensorGroup {
        id: "memory".into(),
        title: "Memory".into(),
        subtitle: Some(subtitle),
        sections,
    }
}

// ---------------------------------------------------------------------------
// Drives
// ---------------------------------------------------------------------------

fn id_safe(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn disk_groups(i: &Inputs) -> Vec<SensorGroup> {
    i.disks
        .iter()
        .map(|d| {
            let sid = id_safe(&d.serial);
            let base = format!("disk/{sid}");
            let mut sections = vec![sec(
                &format!("{base}/io"),
                "Activity",
                false,
                vec![
                    rd(
                        format!("{base}/io/read"),
                        "Read",
                        Unit::BytesPerSec,
                        d.read_bps,
                    ),
                    rd(
                        format!("{base}/io/write"),
                        "Write",
                        Unit::BytesPerSec,
                        d.write_bps,
                    ),
                    rd(
                        format!("{base}/io/active"),
                        "Active time",
                        Unit::Percent,
                        d.active_pct,
                    ),
                ],
            )];
            let chip = d
                .controller
                .as_ref()
                .and_then(|c| chips_named(i.chips, "nvme").find(|ch| &ch.dev == c));
            if let Some(chip) = chip {
                let readings = chip
                    .temps
                    .iter()
                    .map(|t| {
                        let warn = t.max.filter(|m| (30.0..=100.0).contains(m)).unwrap_or(70.0);
                        let crit = t
                            .crit
                            .filter(|c| (50.0..=110.0).contains(c))
                            .unwrap_or(80.0);
                        rd(
                            format!("{base}/temp/{}", slug(&t.label)),
                            t.label.clone(),
                            Unit::Celsius,
                            t.value,
                        )
                        .thr(warn, crit)
                    })
                    .collect();
                sections.push(sec(
                    &format!("{base}/temp"),
                    "Temperatures",
                    false,
                    readings,
                ));
            }
            SensorGroup {
                id: base,
                title: d.model.clone(),
                subtitle: (!d.mounts.is_empty())
                    .then(|| format!("{} · {}", d.model, short_mount(&d.mounts))),
                sections,
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Motherboard and fans
// ---------------------------------------------------------------------------

/// Thermistors that are not wired read far outside this range (or exactly 0). The Super I/O
/// chip has many floating inputs that settle on low values, so its floor is higher.
const BOARD_RANGE: (f64, f64) = (5.0, 115.0);
const SUPERIO_RANGE: (f64, f64) = (15.0, 115.0);

fn board_temp(
    i: &Inputs,
    id_prefix: &str,
    chip: &str,
    t: &TempVal,
    label: &str,
    range: (f64, f64),
) -> Option<Reading> {
    let v = t.value.filter(|v| (range.0..=range.1).contains(v))?;
    let label = i.cfg.label(chip, &t.label).unwrap_or(label);
    let mut r = rd(
        format!("{id_prefix}/{}", slug(&t.label)),
        label,
        Unit::Celsius,
        Some(v),
    );
    r.warn = t.max.filter(|m| (30.0..=100.0).contains(m));
    r.crit = t.crit.filter(|c| (50.0..=110.0).contains(c));
    Some(r)
}

fn board_group(i: &Inputs) -> SensorGroup {
    let mut temps = Vec::new();
    let mut superio = Vec::new();
    let ec = chips_named(i.chips, "asusec").next();
    for label in ["Motherboard", "VRM"] {
        if let Some(t) = temp(ec, label) {
            temps.extend(board_temp(
                i,
                "board/temp/asusec",
                "asusec",
                t,
                label,
                BOARD_RANGE,
            ));
        }
    }
    if let Some(sio) = i.chips.iter().find(|c| hwmon::is_superio(&c.name)) {
        let chip = sio.name.as_str();
        for label in ["SYSTIN", "CPUTIN"] {
            if let Some(t) = temp(Some(sio), label) {
                temps.extend(board_temp(
                    i,
                    &format!("board/temp/{chip}"),
                    chip,
                    t,
                    label,
                    SUPERIO_RANGE,
                ));
            }
        }
        // Nobody knows what the AUXTIN inputs are wired to: listed apart, collapsed.
        for t in sio.temps.iter().filter(|t| t.label.starts_with("AUXTIN")) {
            superio.extend(board_temp(
                i,
                "board/superio",
                chip,
                t,
                &t.label,
                SUPERIO_RANGE,
            ));
        }
    }
    for wifi in i.chips.iter().filter(|c| hwmon::is_wifi(&c.name)) {
        if let Some(t) = wifi.temps.first() {
            let mut t = t.clone();
            t.label = "temp1".into();
            let prefix = format!("board/temp/{}", wifi.name);
            temps.extend(board_temp(
                i,
                &prefix,
                &wifi.name,
                &t,
                "Wi-Fi card",
                BOARD_RANGE,
            ));
        }
    }

    let mut fans = Vec::new();
    // Super I/O chips first, then the board's embedded controller and the laptop fan drivers
    let mut fan_chips: Vec<&ChipVal> = i.chips.iter().filter(|c| hwmon::has_board_fans(&c.name)).collect();
    fan_chips.sort_by_key(|c| !hwmon::is_superio(&c.name));
    for chip in fan_chips {
        let chip_name = chip.name.as_str();
        for f in &chip.fans {
            let key = f.label.clone().unwrap_or_else(|| format!("fan{}", f.idx));
            let configured = i.cfg.label(chip_name, &key);
            if f.rpm == Some(0.0) && configured.is_none() && f.label.is_none() {
                continue; // header with nothing plugged in
            }
            let label = configured
                .map(str::to_string)
                .or_else(|| f.label.clone())
                .unwrap_or_else(|| format!("Fan {}", f.idx));
            fans.push(rd(
                format!("board/fan/{chip_name}/{}", slug(&key)),
                label,
                Unit::Rpm,
                f.rpm,
            ));
        }
    }

    SensorGroup {
        id: "board".into(),
        title: "Motherboard".into(),
        subtitle: Some(i.board_name.to_string()).filter(|s| !s.is_empty()),
        sections: vec![
            sec("board/temp", "Temperatures", false, temps),
            sec(
                "board/superio",
                "Super I/O inputs (unlabeled)",
                true,
                superio,
            ),
            sec("board/fan", "Fans", false, fans),
        ],
    }
}

// ---------------------------------------------------------------------------
// Integrated GPU
// ---------------------------------------------------------------------------

fn igpu_group(i: &Inputs) -> Option<SensorGroup> {
    let g = i.gpus.iter().find(|g| g.card == "igpu")?;
    let mut readings = Vec::new();
    if let Some(t) = g.edge {
        readings.push(rd("igpu/main/temp", "Edge temperature", Unit::Celsius, t.value));
    }
    if g.caps.busy {
        readings.push(rd("igpu/main/usage", "Usage", Unit::Percent, g.busy_pct));
    }
    // Graphics without a temperature or load counter (Intel) at least shows its clock.
    if readings.is_empty() && g.caps.clock_core {
        readings.push(rd("igpu/main/clock", "Clock", Unit::Mhz, g.clock_core_mhz));
    }
    Some(SensorGroup {
        id: "igpu".into(),
        title: "Integrated GPU".into(),
        subtitle: Some(g.name.clone()),
        sections: vec![sec("igpu/main", "Sensors", true, readings)],
    })
}

/// A temperature reading with the warning level the driver's limit implies (10 degrees below it).
fn gpu_temp(id: String, label: &str, t: Option<GpuTemp>) -> Option<Reading> {
    let t = t?;
    let mut r = rd(id, label, Unit::Celsius, t.value);
    if let Some(c) = t.crit.filter(|c| (50.0..=125.0).contains(c)) {
        r = r.thr(c - 10.0, c);
    }
    Some(r)
}

/// One card per discrete AMD or Intel GPU (`gpu1`, `gpu2`, ...). The reading ids follow the
/// NVIDIA card's (`gpu0/temp/core`, `gpu0/load/core`, ...).
fn gpu_cards(i: &Inputs) -> Vec<SensorGroup> {
    let cards: Vec<&CardVal> = i.gpus.iter().filter(|g| g.kind == Kind::Discrete).collect();
    let first = i.gpu.is_some() as usize;
    cards
        .iter()
        .enumerate()
        .map(|(n, g)| {
            let p = g.card.as_str();
            let c = g.caps;
            let mut load = Vec::new();
            if c.busy {
                load.push(rd(format!("{p}/load/core"), "Core", Unit::Percent, g.busy_pct));
            }
            if c.mem_busy {
                load.push(rd(format!("{p}/load/memctl"), "Memory controller", Unit::Percent, g.mem_busy_pct));
            }
            let temps: Vec<Reading> = [
                gpu_temp(format!("{p}/temp/core"), "Edge", g.edge),
                gpu_temp(format!("{p}/temp/hotspot"), "Hotspot", g.junction),
                gpu_temp(format!("{p}/temp/vram"), "VRAM", g.mem),
            ]
            .into_iter()
            .flatten()
            .collect();
            let mut clocks = Vec::new();
            if c.clock_core {
                clocks.push(rd(format!("{p}/clock/core"), "Core", Unit::Mhz, g.clock_core_mhz));
            }
            if c.clock_mem {
                clocks.push(rd(format!("{p}/clock/mem"), "Memory", Unit::Mhz, g.clock_mem_mhz));
            }
            let mut mem = Vec::new();
            if c.vram {
                let pct = match (g.vram_used, g.vram_total) {
                    (Some(u), Some(t)) if t > 0.0 => Some(100.0 * u / t),
                    _ => None,
                };
                mem.push(rd(format!("{p}/mem/used"), "VRAM used", Unit::Bytes, g.vram_used));
                mem.push(rd(format!("{p}/mem/usage"), "VRAM usage", Unit::Percent, pct));
            }
            let mut power = Vec::new();
            if c.power {
                power.push(rd(format!("{p}/power/draw"), "Power draw", Unit::Watt, g.power_w));
            }
            if c.power_cap {
                power.push(rd(format!("{p}/power/limit"), "Power limit", Unit::Watt, g.power_cap_w));
            }
            if c.voltage {
                power.push(rd(format!("{p}/power/voltage"), "Core voltage", Unit::Volt, g.voltage_v));
            }
            if c.fan_rpm {
                power.push(rd(format!("{p}/fan/rpm"), "Fan speed", Unit::Rpm, g.fan_rpm));
            }
            if c.fan_pct {
                power.push(rd(format!("{p}/fan/pct"), "Fan", Unit::Percent, g.fan_pct));
            }
            let mut subtitle = match g.vram_total {
                Some(t) if c.vram => format!("{} · {:.0} GB", g.name, t / GIB),
                _ => g.name.clone(),
            };
            if g.asleep {
                subtitle.push_str(" · asleep");
            }
            let title = if first + cards.len() == 1 {
                "GPU".to_string()
            } else {
                format!("GPU {}", first + n + 1)
            };
            SensorGroup {
                id: g.card.clone(),
                title,
                subtitle: Some(subtitle),
                sections: vec![
                    sec(&format!("{p}/load"), "Usage", false, load),
                    sec(&format!("{p}/temp"), "Temperatures", false, temps),
                    sec(&format!("{p}/clock"), "Clocks", false, clocks),
                    sec(&format!("{p}/mem"), "Memory", false, mem),
                    sec(&format!("{p}/power"), "Power and fan", false, power),
                ],
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Summary
// ---------------------------------------------------------------------------

pub fn summary(groups: &[SensorGroup], i: &Inputs, unix_ms: u64) -> Summary {
    let mut vals: HashMap<&str, f64> = HashMap::new();
    for r in groups
        .iter()
        .flat_map(|g| &g.sections)
        .flat_map(|s| &s.readings)
    {
        if let Some(v) = r.value {
            vals.insert(&r.id, v);
        }
    }
    let get = |id: &str| vals.get(id).copied();
    let ids = |pred: &dyn Fn(&str) -> bool| -> Vec<f64> {
        vals.iter()
            .filter(|(k, _)| pred(k))
            .map(|(_, v)| *v)
            .collect()
    };
    // The NVIDIA card, else the first discrete one: what the bar widget shows as "the GPU".
    let gp = groups
        .iter()
        .find(|g| is_gpu_card(&g.id))
        .map(|g| g.id.as_str())
        .unwrap_or("gpu0");
    let gget = |tail: &str| get(&format!("{gp}/{tail}"));
    let vram_total = if gp == "gpu0" {
        i.gpu.and_then(|g| g.vram_total)
    } else {
        i.gpus.iter().find(|g| g.card == gp).and_then(|g| g.vram_total)
    };
    let hottest = |v: Vec<f64>| max(v.into_iter());
    let sum = |v: Vec<f64>| {
        if v.is_empty() {
            None
        } else {
            Some(v.iter().sum())
        }
    };
    Summary {
        unix_ms,
        cpu_usage_pct: get("cpu/usage/total"),
        cpu_temp_c: get("cpu/temp/tctl").or_else(|| get("cpu/temp/package")),
        cpu_power_w: get("cpu/power/package"),
        cpu_clock_avg_mhz: get("cpu/clock/avg"),
        gpu_usage_pct: gget("load/core"),
        gpu_temp_c: gget("temp/core"),
        gpu_hotspot_c: gget("temp/hotspot"),
        gpu_vram_temp_c: gget("temp/vram"),
        gpu_power_w: gget("power/draw"),
        gpu_vram_used_bytes: gget("mem/used"),
        gpu_vram_total_bytes: vram_total,
        ram_used_pct: get("memory/usage/pct"),
        ram_used_bytes: get("memory/usage/used"),
        ram_total_bytes: i.mem.map(|m| m.total as f64),
        swap_total_bytes: i.mem.map(|m| m.swap_total as f64).filter(|b| *b > 0.0),
        ram_temp_max_c: hottest(ids(&|id| id.starts_with("memory/temp/dimm"))),
        ssd_temp_max_c: hottest(ids(&|id| {
            id.starts_with("disk/") && id.ends_with("/temp/composite")
        })),
        disk_read_bps: sum(ids(&|id| {
            id.starts_with("disk/") && id.ends_with("/io/read")
        })),
        disk_write_bps: sum(ids(&|id| {
            id.starts_with("disk/") && id.ends_with("/io/write")
        })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hwmon::FanVal;
    use crate::gpu::{CardVal, GpuTemp, Kind};

    fn cfg() -> Config {
        Config::defaults()
    }

    fn base_inputs<'a>(cfg: &'a Config, cpu: &'a CpuVal, chips: &'a [ChipVal]) -> Inputs<'a> {
        Inputs {
            cfg,
            cpu,
            chips,
            mem: None,
            disks: &[],
            gpu: None,
            gpus: &[],
            lact: None,
            helper_power: None,
            helper_mem: None,
            board_name: "",
            ppt_estimate: None,
        }
    }

    fn tv(label: &str, v: f64) -> TempVal {
        TempVal {
            label: label.into(),
            value: Some(v),
            max: None,
            crit: None,
        }
    }

    #[test]
    fn unconnected_sensors_and_idle_fans_are_hidden() {
        let cfg = cfg();
        let cpu = CpuVal::default();
        let chips = vec![ChipVal {
            name: "nct6799".into(),
            dev: "nct6775.656".into(),
            temps: vec![
                tv("SYSTIN", 43.0),
                tv("AUXTIN3", -62.0),
                tv("PCH_CPU_TEMP", 0.0),
                tv("AUXTIN4", 24.0),
                tv("AUXTIN5", 12.0),
            ],
            fans: vec![
                FanVal {
                    idx: 1,
                    label: None,
                    rpm: Some(1100.0),
                },
                FanVal {
                    idx: 3,
                    label: None,
                    rpm: Some(0.0),
                },
                FanVal {
                    idx: 7,
                    label: None,
                    rpm: Some(0.0),
                },
            ],
        }];
        let g = board_group(&base_inputs(&cfg, &cpu, &chips));
        let temps: Vec<&str> = g.sections[0]
            .readings
            .iter()
            .map(|r| r.id.as_str())
            .collect();
        assert_eq!(temps, ["board/temp/nct6799/systin"]);
        // below 15 C counts as unconnected; unlabeled inputs live in their own collapsed section
        assert_eq!(g.sections[1].id, "board/superio");
        assert!(g.sections[1].collapsed_by_default);
        let superio: Vec<&str> = g.sections[1]
            .readings
            .iter()
            .map(|r| r.id.as_str())
            .collect();
        assert_eq!(superio, ["board/superio/auxtin4"]);
        let fans: Vec<(&str, &str)> = g.sections[2]
            .readings
            .iter()
            .map(|r| (r.id.as_str(), r.label.as_str()))
            .collect();
        // no built-in names: fan1 is "Fan 1", fan3 and fan7 read 0 and have no name
        assert_eq!(fans, [("board/fan/nct6799/fan1", "Fan 1")]);
        // a name from config.toml shows the fan even at 0 RPM
        let cfg = Config::for_test(&[("nct6799/fan1", "Top intake"), ("nct6799/fan3", "Rear")], None);
        let g = board_group(&base_inputs(&cfg, &cpu, &chips));
        let fans: Vec<(&str, &str)> = g.sections[2]
            .readings
            .iter()
            .map(|r| (r.id.as_str(), r.label.as_str()))
            .collect();
        assert_eq!(
            fans,
            [("board/fan/nct6799/fan1", "Top intake"), ("board/fan/nct6799/fan3", "Rear")]
        );
    }

    fn fan(idx: u32, label: Option<&str>, rpm: f64) -> FanVal {
        FanVal { idx, label: label.map(Into::into), rpm: Some(rpm) }
    }

    #[test]
    fn fans_use_the_driver_label_then_fan_n_on_any_super_io_chip() {
        let cfg = cfg();
        let cpu = CpuVal::default();
        let chips = vec![
            ChipVal {
                name: "asusec".into(),
                fans: vec![fan(1, Some("CPU_Opt"), 0.0)],
                ..Default::default()
            },
            ChipVal {
                name: "it8688".into(),
                temps: vec![tv("SYSTIN", 31.0)],
                fans: vec![fan(1, None, 900.0), fan(2, Some("Pump"), 2700.0)],
                ..Default::default()
            },
        ];
        let g = board_group(&base_inputs(&cfg, &cpu, &chips));
        let fans: Vec<(&str, &str)> = g.sections[2]
            .readings
            .iter()
            .map(|r| (r.id.as_str(), r.label.as_str()))
            .collect();
        // Super I/O chips come first; a driver label keeps a stopped fan on the list
        assert_eq!(
            fans,
            [
                ("board/fan/it8688/fan1", "Fan 1"),
                ("board/fan/it8688/pump", "Pump"),
                ("board/fan/asusec/cpu_opt", "CPU_Opt")
            ]
        );
        assert_eq!(g.sections[0].readings[0].id, "board/temp/it8688/systin");
        // config.toml renames by chip and key
        let cfg = Config::for_test(&[("asusec/CPU_Opt", "CPU OPT"), ("it8688/fan1", "Top")], None);
        let g = board_group(&base_inputs(&cfg, &cpu, &chips));
        let labels: Vec<&str> = g.sections[2].readings.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(labels, ["Top", "Pump", "CPU OPT"]);
    }

    fn cpu_with_temps(model: &str, chips: &[ChipVal], cfg: &Config, cpu: CpuVal) -> Vec<Reading> {
        let cpu = CpuVal { model: model.into(), ..cpu };
        let g = cpu_group(&base_inputs(cfg, &cpu, chips));
        g.sections.into_iter().flat_map(|s| s.readings).collect()
    }

    fn find<'a>(r: &'a [Reading], id: &str) -> &'a Reading {
        r.iter().find(|r| r.id == id).unwrap_or_else(|| panic!("no {id}"))
    }

    fn k10(tctl: f64) -> Vec<ChipVal> {
        vec![ChipVal { name: "k10temp".into(), temps: vec![tv("Tctl", tctl), tv("Tccd1", tctl - 5.0)], ..Default::default() }]
    }

    #[test]
    fn amd_throttle_limit_comes_from_config_driver_table_or_default() {
        let model = |m: &str, tctl: f64, cfg: &Config| cpu_with_temps(m, &k10(tctl), cfg, CpuVal { model_limit_c: crate::cpu::amd_tjmax(m), ..Default::default() });
        let c = cfg();
        // the 7800X3D: 89 from the table
        let r = model("AMD Ryzen 7 7800X3D 8-Core Processor", 89.0, &c);
        assert_eq!(find(&r, "cpu/throttle/thermal").value, Some(1.0));
        assert_eq!(find(&r, "cpu/throttle/thermal").note.as_deref(), Some("inferred from Tctl"));
        assert_eq!((find(&r, "cpu/temp/tctl").warn, find(&r, "cpu/temp/tctl").crit), (Some(80.0), Some(89.0)));
        let r = model("AMD Ryzen 7 7800X3D 8-Core Processor", 85.0, &c);
        assert_eq!(find(&r, "cpu/throttle/thermal").value, Some(0.0));
        // a 7950X at 89 is not throttling: its limit is 95
        let r = model("AMD Ryzen 9 7950X 16-Core Processor", 89.0, &c);
        assert_eq!(find(&r, "cpu/throttle/thermal").value, Some(0.0));
        assert_eq!(find(&r, "cpu/temp/tctl").crit, Some(95.0));
        // an unknown model: 95
        let r = model("Some Future CPU", 94.6, &c);
        assert_eq!(find(&r, "cpu/throttle/thermal").value, Some(1.0));
        // the user's setting beats the table
        let c = Config::for_test(&[], Some(80.0));
        let r = model("AMD Ryzen 7 7800X3D 8-Core Processor", 80.0, &c);
        assert_eq!(find(&r, "cpu/throttle/thermal").value, Some(1.0));
        assert_eq!(find(&r, "cpu/temp/tccd1").crit, Some(80.0));
        // a limit the driver reports beats the table
        let mut chips = k10(100.0);
        chips[0].temps[0].crit = Some(100.0);
        let r = cpu_with_temps("AMD Ryzen 7 7800X3D 8-Core Processor", &chips, &cfg(), CpuVal { model_limit_c: Some(89.0), ..Default::default() });
        assert_eq!(find(&r, "cpu/throttle/thermal").value, Some(1.0));
        assert_eq!(find(&r, "cpu/temp/tctl").crit, Some(100.0));
        // Tdie, when the chip has an offset, is what is compared with the limit
        let mut chips = k10(115.0);
        chips[0].temps.push(tv("Tdie", 70.0));
        let r = cpu_with_temps("AMD Ryzen 7 1700X Eight-Core Processor", &chips, &cfg(), CpuVal { model_limit_c: Some(95.0), ..Default::default() });
        assert_eq!(find(&r, "cpu/throttle/thermal").value, Some(0.0));
    }

    fn coretemp() -> Vec<ChipVal> {
        let mut pkg = tv("Package id 0", 55.0);
        pkg.crit = Some(100.0);
        pkg.max = Some(100.0);
        vec![ChipVal {
            name: "coretemp".into(),
            temps: vec![pkg, tv("Core 0", 50.0), tv("Core 1", 52.0), tv("Core 8", 61.0)],
            ..Default::default()
        }]
    }

    #[test]
    fn intel_cpus_get_package_and_core_temperatures_and_real_throttling() {
        let cfg = cfg();
        let intel = CpuVal { throttling: Some(false), ..Default::default() };
        let r = cpu_with_temps("12th Gen Intel(R) Core(TM) i7-12700K", &coretemp(), &cfg, intel.clone());
        let ids: Vec<&str> = r.iter().filter(|r| r.id.starts_with("cpu/temp/")).map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["cpu/temp/package", "cpu/temp/core0", "cpu/temp/core1", "cpu/temp/core8"]);
        // the limit is the driver's Tjmax
        assert_eq!((find(&r, "cpu/temp/package").warn, find(&r, "cpu/temp/package").crit), (Some(91.0), Some(100.0)));
        let t = find(&r, "cpu/throttle/thermal");
        assert_eq!((t.value, t.note.as_deref()), (Some(0.0), Some("from the CPU's thermal throttle counters")));
        // a counter that rose means throttling, whatever the temperature
        let hot = CpuVal { throttling: Some(true), ..Default::default() };
        let r = cpu_with_temps("Intel", &coretemp(), &cfg, hot);
        assert_eq!(find(&r, "cpu/throttle/thermal").value, Some(1.0));
        // no counters: inferred from the package temperature against Tjmax
        let mut chips = coretemp();
        chips[0].temps[0].value = Some(99.8);
        let r = cpu_with_temps("Intel", &chips, &cfg, CpuVal::default());
        let t = find(&r, "cpu/throttle/thermal");
        assert_eq!((t.value, t.note.as_deref()), (Some(1.0), Some("inferred from Package id 0")));
        // a second package gets its own ids
        let mut two = coretemp();
        two.push(coretemp().remove(0));
        let r = cpu_with_temps("Intel", &two, &cfg, intel);
        assert_eq!(find(&r, "cpu/temp/package1").label, "Package 1");
        assert_eq!(find(&r, "cpu/temp/p1core0").label, "Core 0");
    }

    #[test]
    fn summary_cpu_temperature_falls_back_to_the_package() {
        let cfg = cfg();
        let cpu = CpuVal::default();
        let chips = coretemp();
        let i = base_inputs(&cfg, &cpu, &chips);
        let groups = build(&i);
        assert_eq!(summary(&groups, &i, 0).cpu_temp_c, Some(55.0));
    }

    fn card(id: &str, kind: Kind) -> CardVal {
        use crate::gpu::Caps;
        CardVal {
            card: id.into(),
            name: "AMD Radeon RX 7900 XTX".into(),
            kind,
            caps: Caps { busy: true, mem_busy: true, vram: true, power: true, power_cap: true, fan_rpm: true, fan_pct: true, clock_core: true, clock_mem: true, voltage: true },
            asleep: false,
            edge: Some(GpuTemp { value: Some(50.0), crit: Some(100.0) }),
            junction: Some(GpuTemp { value: Some(64.0), crit: Some(110.0) }),
            mem: Some(GpuTemp { value: Some(70.0), crit: None }),
            busy_pct: Some(80.0),
            mem_busy_pct: Some(20.0),
            vram_used: Some(8.0 * GIB),
            vram_total: Some(24.0 * GIB),
            power_w: Some(250.0),
            power_cap_w: Some(327.0),
            fan_rpm: Some(1450.0),
            fan_pct: Some(40.0),
            clock_core_mhz: Some(2400.0),
            clock_mem_mhz: Some(1249.0),
            voltage_v: Some(1.05),
        }
    }

    #[test]
    fn integrated_graphics_card_is_named_from_the_system() {
        let cfg = cfg();
        let cpu = CpuVal::default();
        let mut igpu = card("igpu", Kind::Integrated);
        igpu.name = "AMD Radeon Graphics (Raphael)".into();
        igpu.junction = None;
        igpu.mem = None;
        let gpus = [igpu];
        let mut i = base_inputs(&cfg, &cpu, &[]);
        i.gpus = &gpus;
        let g = igpu_group(&i).unwrap();
        assert_eq!((g.id.as_str(), g.title.as_str()), ("igpu", "Integrated GPU"));
        assert_eq!(g.subtitle.as_deref(), Some("AMD Radeon Graphics (Raphael)"));
        let ids: Vec<&str> = g.sections[0].readings.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["igpu/main/temp", "igpu/main/usage"]);
        // no integrated card, no group; a discrete card is not the "Integrated GPU"
        let gpus = [card("gpu1", Kind::Discrete)];
        i.gpus = &gpus;
        assert!(igpu_group(&i).is_none());
    }

    #[test]
    fn discrete_amd_card_has_its_own_card_and_stable_ids() {
        let cfg = cfg();
        let cpu = CpuVal::default();
        let gpus = [card("gpu1", Kind::Discrete), card("igpu", Kind::Integrated)];
        let mut i = base_inputs(&cfg, &cpu, &[]);
        i.gpus = &gpus;
        let groups = build(&i);
        let ids: Vec<&str> = groups.iter().map(|g| g.id.as_str()).collect();
        assert!(ids.contains(&"gpu1") && ids.contains(&"igpu") && !ids.contains(&"gpu0"), "{ids:?}");
        let g = groups.iter().find(|g| g.id == "gpu1").unwrap();
        // the only GPU card: plain "GPU", with the VRAM size in the subtitle
        assert_eq!((g.title.as_str(), g.subtitle.as_deref()), ("GPU", Some("AMD Radeon RX 7900 XTX · 24 GB")));
        let sections: Vec<&str> = g.sections.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(sections, ["gpu1/temp", "gpu1/load", "gpu1/power", "gpu1/clock", "gpu1/mem"]);
        let all: Vec<&Reading> = g.sections.iter().flat_map(|s| &s.readings).collect();
        let get = |id: &str| all.iter().find(|r| r.id == id).unwrap_or_else(|| panic!("no {id}"));
        assert_eq!((get("gpu1/temp/hotspot").value, get("gpu1/temp/hotspot").crit, get("gpu1/temp/hotspot").warn), (Some(64.0), Some(110.0), Some(100.0)));
        assert_eq!(get("gpu1/temp/vram").crit, None);
        assert_eq!(get("gpu1/mem/usage").value.map(|v| v.round()), Some(33.0));
        assert_eq!(get("gpu1/power/limit").value, Some(327.0));
        assert_eq!(get("gpu1/fan/rpm").value, Some(1450.0));
        assert_eq!(get("gpu1/clock/mem").value, Some(1249.0));
        // every reading has a description and the sections summarize
        assert!(all.iter().all(|r| r.desc.is_some()), "{:?}", all.iter().filter(|r| r.desc.is_none()).map(|r| &r.id).collect::<Vec<_>>());
        assert_eq!(g.sections[0].summary.as_ref().unwrap().id, "gpu1/temp/hotspot");
        // the summary (bar widget) shows this card when there is no NVIDIA one
        let s = summary(&groups, &i, 0);
        assert_eq!((s.gpu_usage_pct, s.gpu_hotspot_c, s.gpu_power_w), (Some(80.0), Some(64.0), Some(250.0)));
        assert_eq!(s.gpu_vram_total_bytes, Some(24.0 * GIB));
        // a sleeping card says so and is not read
        let mut asleep = card("gpu1", Kind::Discrete);
        asleep.asleep = true;
        let gpus = [asleep];
        i.gpus = &gpus;
        let g = gpu_cards(&i).remove(0);
        assert_eq!(g.subtitle.as_deref(), Some("AMD Radeon RX 7900 XTX · 24 GB · asleep"));
    }

    #[test]
    fn card_titles_are_numbered_when_there_are_several() {
        let cfg = cfg();
        let cpu = CpuVal::default();
        let gpus = [card("gpu1", Kind::Discrete), card("gpu2", Kind::Discrete)];
        let mut i = base_inputs(&cfg, &cpu, &[]);
        i.gpus = &gpus;
        let titles: Vec<String> = gpu_cards(&i).into_iter().map(|g| g.title).collect();
        assert_eq!(titles, ["GPU 1", "GPU 2"]);
    }

    #[test]
    fn cpu_threads_are_laid_out_per_core() {
        let cfg = cfg();
        let mut cpu = CpuVal {
            model: "Test CPU".into(),
            total: Some(10.0),
            ..Default::default()
        };
        for core in 0..2 {
            for t in 0..2 {
                cpu.threads.push(ThreadVal {
                    core,
                    t,
                    usage: Some(10.0 * (core + t) as f64),
                    mhz: Some(4000.0),
                });
            }
        }
        let g = cpu_group(&base_inputs(&cfg, &cpu, &[]));
        let ids: Vec<&str> = g.sections[2]
            .readings
            .iter()
            .map(|r| r.id.as_str())
            .collect();
        assert_eq!(ids[0], "cpu/thread/c0t0/usage");
        assert_eq!(ids[3], "cpu/thread/c1t1/usage");
        assert_eq!(ids[4], "cpu/thread/c0t0/clock");
        let c1 = g.sections[0]
            .readings
            .iter()
            .find(|r| r.id == "cpu/usage/core1")
            .unwrap();
        assert_eq!(c1.value, Some(15.0)); // mean of 10 and 20
    }

    #[test]
    fn short_names() {
        assert_eq!(
            short_cpu_name("AMD Ryzen 7 7800X3D 8-Core Processor"),
            "AMD Ryzen 7 7800X3D"
        );
        assert_eq!(
            short_cpu_name("Intel(R) Core(TM) i7"),
            "Intel(R) Core(TM) i7"
        );
        assert_eq!(short_vendor("G Skill Intl"), "G.Skill");
        let m = |v: &[&str]| short_mount(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(m(&["/", "/boot", "/home"]), "/");
        assert_eq!(m(&["/boot", "/home"]), "boot");
        assert_eq!(m(&["/run/media/alex/games"]), "games");
        assert_eq!(m(&["/mnt/data/backup/"]), "backup");
    }

    #[test]
    fn sections_are_ordered_and_collapsed() {
        let mk = |id: &str, collapsed: bool| sec(id, id, collapsed, vec![]);
        let mut g = SensorGroup {
            id: "cpu".into(),
            title: "CPU".into(),
            subtitle: None,
            sections: [
                "cpu/usage",
                "cpu/clock",
                "cpu/thread",
                "cpu/temp",
                "cpu/limits",
                "cpu/power",
            ]
            .iter()
            .map(|id| mk(id, false))
            .collect(),
        };
        arrange(&mut g);
        let ids: Vec<(&str, bool)> = g
            .sections
            .iter()
            .map(|s| (s.id.as_str(), s.collapsed_by_default))
            .collect();
        assert_eq!(
            ids,
            [
                ("cpu/temp", false),
                ("cpu/limits", false),
                ("cpu/power", false),
                ("cpu/usage", true),
                ("cpu/clock", true),
                ("cpu/thread", true)
            ]
        );
    }
}
