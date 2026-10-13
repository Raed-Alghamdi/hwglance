//! Synthetic, slowly changing snapshots that look like the real PC (`hwglance --demo`).

use hwglance_proto::{
    crossings::Tracker, ClientMsg, FlagEvents, HistorySeries, LimitOverride, ProcClass, ProcGroup, ProcInfo, Reading, Section, SensorGroup, Snapshot, SourceStatus, Stats,
    Summary, Unit, PROTO_VERSION,
};
use std::collections::{BTreeMap, HashMap, VecDeque};

/// Demo time of "now" when the demo starts: history exists for the 30 minutes before it.
pub const T_START: f64 = 2000.0;
const HISTORY_S: f64 = 1800.0;

#[derive(Default, Clone, Copy)]
struct FlagState {
    on: bool,
    count: u32,
    last_ms: u64,
    ended_ms: u64,
}

/// Throttling that sometimes turns on, on a fixed schedule so the demo is repeatable.
pub fn cpu_thermal_on(t: f64) -> bool {
    (560.0..570.0).contains(&(t % 600.0))
}
pub fn gpu_thermal_on(t: f64) -> bool {
    (800.0..808.0).contains(&(t % 900.0))
}
pub fn gpu_power_on(t: f64) -> bool {
    (380.0..388.0).contains(&(t % 420.0))
}

const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

pub struct Demo {
    stats: HashMap<String, Stats>,
    since_ms: u64,
    seq: u64,
    start_unix_ms: u64,
    cur_ms: u64,
    flags: HashMap<String, FlagState>,
    history: HashMap<String, (Unit, VecDeque<(u64, f32)>)>,
    /// Threshold crossings, recorded by the collector's own rules.
    crossings: Tracker,
    /// Limits the window set, as the collector would keep them: (warn, crit, below) by reading id.
    limits: HashMap<String, (Option<f64>, Option<f64>, bool)>,
    /// The ones of them in force in the snapshot being built.
    shown: BTreeMap<String, LimitOverride>,
}

fn hash(s: &str, k: u64) -> f64 {
    let mut h: u64 = 0xcbf29ce484222325 ^ k.wrapping_mul(0x9e3779b97f4a7c15);
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h ^= h >> 29;
    h = h.wrapping_mul(0xbf58476d1ce4e5b9);
    h ^= h >> 32;
    (h % 1_000_000) as f64 / 1_000_000.0
}

/// Smooth noise in 0..1, changing over a few seconds.
fn noise(key: &str, t: f64) -> f64 {
    let step = 6.0;
    let i = (t / step).floor();
    let f = (t / step) - i;
    let f = f * f * (3.0 - 2.0 * f);
    let a = hash(key, i as u64);
    let b = hash(key, i as u64 + 1);
    a + (b - a) * f
}

fn rd(id: &str, label: &str, unit: Unit, value: f64, warn: Option<f64>, crit: Option<f64>, note: Option<&str>) -> (Reading, f64) {
    (
        Reading {
            id: id.into(),
            label: label.into(),
            unit,
            value: Some(value),
            stats: None,
            note: note.map(Into::into),
            desc: demo_desc(id, label),
            events: None,
            warn,
            crit,
        },
        value,
    )
}

/// Hover text in the style of the real collector's descriptions.
fn demo_desc(id: &str, label: &str) -> Option<String> {
    let text = if id == "cpu/usage/total" {
        "Share of all logical CPUs that is busy, averaged over every thread."
    } else if id == "cpu/usage/max" {
        "Usage of the busiest single thread. Near 100 % while Total is low means a single-threaded bottleneck."
    } else if id.starts_with("cpu/clock/avg") {
        "Average effective clock of all cores: the speed they actually ran at, not the requested one."
    } else if id.starts_with("cpu/clock") {
        "Effective clock of one core. It drops when the core is idle and boosts under load."
    } else if id == "cpu/temp/tctl" {
        "AMD's control temperature, offset on some models. This is what the fans and throttling follow."
    } else if id.starts_with("cpu/temp") {
        "Temperature of one core complex die. Usually a few degrees under Tctl."
    } else if id == "cpu/power/package" {
        "Power drawn by the whole CPU package: cores, cache and the I/O die. Read from RAPL energy counters."
    } else if id.starts_with("cpu/power") {
        "Part of the CPU package power, from RAPL energy counters."
    } else if id.starts_with("cpu/volt") {
        "Voltage the CPU requests from the board. It rises with load and boost."
    } else if id.starts_with("cpu/thread") {
        "Per-thread value. Each core has two threads (SMT)."
    } else if id == "gpu0/load/core" {
        "How busy the GPU's shaders are, as NVML reports it. A game at 100 % here is GPU-bound."
    } else if id.starts_with("gpu0/load") {
        "Usage of a fixed-function unit of the GPU (memory controller or video engine)."
    } else if id == "gpu0/temp/hotspot" {
        "Hottest point on the GPU die. It runs several degrees above the core temperature."
    } else if id == "gpu0/temp/vram" {
        "Hottest GDDR6X chip. Memory throttles near 105 C, well before the core does."
    } else if id.starts_with("gpu0/temp") {
        "GPU core temperature, the value the fan curve follows."
    } else if id.starts_with("gpu0/chip") {
        "Temperature of one GDDR6X memory chip on the card."
    } else if id.starts_with("gpu0/clock") {
        "Current clock of this GPU domain. It idles low and boosts under load."
    } else if id.starts_with("gpu0/power") {
        "Power the card draws, measured on its own sensors. It includes memory and fans."
    } else if id.starts_with("gpu0/fan") {
        "Fan speed of the card. Zero means the fans have stopped, which is normal at idle."
    } else if id.starts_with("gpu0/mem") {
        "Video memory in use on the GPU, including what other programs hold."
    } else if id.starts_with("memory/temp") {
        "Temperature reported by the module's own sensor. DDR5 modules run warm under load."
    } else if id.starts_with("memory/") {
        "System memory figure from /proc/meminfo. Cache is reclaimable, so Available matters more than Free."
    } else if id.contains("/io/read") || id.contains("/io/write") {
        "Throughput of the drive in the last interval, from the kernel's block layer counters."
    } else if id.contains("/temp/") && id.starts_with("nvme") {
        "Drive temperature from its SMART data. Sustained values above 70 C can slow the drive down."
    } else if id.starts_with("nvme") {
        "Drive figure from SMART data and the filesystem."
    } else if id.starts_with("mb/fan") {
        "Fan or pump speed reported by the Super I/O chip. A pump at 0 RPM means the loop has stopped."
    } else if id.starts_with("mb/temp") {
        "Board temperature from the Super I/O chip or the embedded controller."
    } else if id.starts_with("mb/volt") {
        "Supply voltage measured by the Super I/O chip. Small deviations are normal."
    } else {
        return Some(format!("{label}."));
    };
    Some(text.to_string())
}

/// Section order and default collapse as the collector arranges them: temperatures and limits
/// first, the per-core, per-thread and per-chip lists collapsed.
fn arrange(g: &mut SensorGroup) {
    const COLLAPSED: &[&str] = &["cpu/usage", "cpu/clock", "cpu/thread", "gpu0/chips"];
    let order: &[&str] = match g.id.as_str() {
        "cpu" => &["temp", "limits", "power", "usage", "clock", "thread"],
        "gpu0" => &["temp", "limits", "load", "clocks", "fan", "mem", "chips"],
        "memory" => &["temp", "usage", "speed"],
        "mb" => &["temp", "fan", "volt", "misc"],
        id if id.starts_with("nvme") => &["temp", "io", "cap"],
        _ => &[],
    };
    if !order.is_empty() {
        let card = g.id.clone();
        g.sections.sort_by_key(|s| {
            let tail = s.id.strip_prefix(&format!("{card}/")).unwrap_or(&s.id);
            order.iter().position(|o| *o == tail).unwrap_or(order.len())
        });
    }
    for s in &mut g.sections {
        if g.id != "igpu" {
            s.collapsed_by_default = COLLAPSED.contains(&s.id.as_str());
        }
    }
}

impl Default for Demo {
    fn default() -> Self {
        Self::new()
    }
}

impl Demo {
    pub fn new() -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        Self::at(now)
    }

    /// A demo whose clock reads `now_ms` (unix milliseconds) at demo time `T_START`, so the
    /// timestamps in its snapshots are fixed (golden-image tests).
    pub fn at(now_ms: u64) -> Self {
        let start = now_ms.saturating_sub((T_START * 1000.0) as u64);
        Demo {
            stats: HashMap::new(),
            since_ms: 0,
            seq: 0,
            start_unix_ms: start,
            cur_ms: start,
            flags: HashMap::new(),
            history: HashMap::new(),
            crossings: Tracker::new(),
            limits: HashMap::new(),
            shown: BTreeMap::new(),
        }
    }

    /// Do what the collector does with a limit request from the window.
    pub fn apply_limit_request(&mut self, msg: &ClientMsg) {
        match msg {
            ClientMsg::SetThreshold { id, warn, crit, below } => {
                self.limits.insert(id.clone(), (*warn, *crit, *below));
            }
            ClientMsg::ResetThreshold { id } => {
                self.limits.remove(id);
            }
            _ => {}
        }
    }

    pub fn reset_stats(&mut self) {
        self.stats.clear();
        self.since_ms = self.cur_ms;
        for f in self.flags.values_mut() {
            f.count = 0;
            f.last_ms = 0;
            f.ended_ms = 0;
        }
        self.crossings.clear_closed();
    }

    /// Run the clock from `from` to `to` (demo seconds) to build up history and flag events.
    pub fn warm_up(&mut self, from: f64, to: f64, step: f64) {
        let mut t = from;
        while t < to {
            self.snapshot(t);
            t += step;
        }
    }

    /// History of the given readings for the last `seconds`.
    pub fn history_reply(&self, ids: &[String], seconds: u32) -> Vec<HistorySeries> {
        ids.iter()
            .filter_map(|id| {
                let (unit, pts) = self.history.get(id)?;
                let newest = pts.back()?.0;
                let from = newest.saturating_sub(seconds as u64 * 1000);
                Some(HistorySeries { id: id.clone(), unit: *unit, points: pts.iter().filter(|p| p.0 >= from).copied().collect() })
            })
            .collect()
    }

    fn finish(&mut self, (mut r, v): (Reading, f64)) -> Reading {
        let st = self.stats.entry(r.id.clone()).and_modify(|s| s.push(v)).or_insert_with(|| Stats::first(v));
        r.stats = Some(*st);
        let ms = self.cur_ms;
        let mut below = false;
        if r.unit != Unit::Flag
            && let Some(&(warn, crit, b)) = self.limits.get(&r.id)
            && (warn, crit, b) != (r.warn, r.crit, false)
        {
            self.shown.insert(r.id.clone(), LimitOverride { warn, crit, below: b, default_warn: r.warn, default_crit: r.crit });
            (r.warn, r.crit, below) = (warn, crit, b);
        }
        self.crossings.observe(ms, &r.id, v, r.warn, r.crit, below);
        let (_, pts) = self.history.entry(r.id.clone()).or_insert_with(|| (r.unit, VecDeque::new()));
        pts.push_back((ms, v as f32));
        while pts.front().is_some_and(|p| ms.saturating_sub(p.0) > (HISTORY_S * 1000.0) as u64) {
            pts.pop_front();
        }
        r
    }

    /// A yes/no reading whose onsets are counted.
    fn flag(&mut self, id: &str, label: &str, on: bool, desc: &str) -> Reading {
        let ms = self.cur_ms;
        let f = self.flags.entry(id.to_string()).or_default();
        if on && !f.on {
            f.count += 1;
        }
        if !on && f.on {
            f.ended_ms = ms;
        }
        if on {
            f.last_ms = ms;
        }
        f.on = on;
        let ev = FlagEvents { count: f.count, last_unix_ms: f.last_ms, ended_unix_ms: (!on && f.ended_ms > 0).then_some(f.ended_ms) };
        let r = Reading {
            id: id.into(),
            label: label.into(),
            unit: Unit::Flag,
            value: Some(if on { 1.0 } else { 0.0 }),
            stats: None,
            note: None,
            desc: Some(desc.into()),
            events: None,
            warn: None,
            crit: None,
        };
        let mut r = self.finish((r, if on { 1.0 } else { 0.0 }));
        r.events = Some(ev);
        r
    }

    fn limits_section(&mut self, id: &str, flags: Vec<(&str, &str, bool, &str)>) -> Section {
        let readings: Vec<Reading> = flags.into_iter().map(|(rid, label, on, desc)| self.flag(rid, label, on, desc)).collect();
        let summary = readings.first().cloned();
        Section { id: id.into(), title: "Limits".into(), collapsed_by_default: false, readings, summary }
    }

    fn derived(&mut self, sec_id: &str, label: &str, unit: Unit, value: f64, desc: &str) -> Reading {
        let r = Reading {
            id: format!("{sec_id}/summary"),
            label: label.into(),
            unit,
            value: Some(value),
            stats: None,
            note: None,
            desc: Some(desc.into()),
            events: None,
            warn: None,
            crit: None,
        };
        self.finish((r, value))
    }

    fn section(&mut self, id: &str, title: &str, collapsed: bool, rows: Vec<(Reading, f64)>) -> Section {
        let readings: Vec<Reading> = rows.into_iter().map(|r| self.finish(r)).collect();
        let max_of = |f: &dyn Fn(&Reading) -> bool| readings.iter().filter(|r| f(r)).filter_map(|r| r.value).fold(f64::MIN, f64::max);
        let all_temps = readings.len() > 1 && readings.iter().all(|r| r.unit == Unit::Celsius);
        let copy = |rid: &str| readings.iter().find(|r| r.id == rid).cloned();
        let summary = match id {
            "cpu/usage" => copy("cpu/usage/total"),
            "cpu/clock" => copy("cpu/clock/avg"),
            "cpu/power" => copy("cpu/power/package"),
            "gpu0/clocks" => copy("gpu0/clock/gfx"),
            "gpu0/mem" => copy("gpu0/mem/used"),
            "memory/usage" => copy("memory/usage/used"),
            "cpu/thread" => {
                let v = max_of(&|r| r.id.ends_with("/usage"));
                Some(self.derived(id, "Busiest thread", Unit::Percent, v, "Usage of the busiest single thread. Near 100 % while Total is low means one thread is the bottleneck, which is common in games."))
            }
            _ if all_temps => {
                let v = max_of(&|_| true);
                Some(self.derived(id, "Hottest", Unit::Celsius, v, "The highest temperature in this section."))
            }
            _ => readings.first().cloned(),
        };
        Section { id: id.into(), title: title.into(), collapsed_by_default: collapsed, readings, summary }
    }

    /// Build the snapshot for `t` seconds of demo time.
    pub fn snapshot(&mut self, t: f64) -> Snapshot {
        self.seq += 1;
        let now_ms = self.start_unix_ms + (t * 1000.0) as u64;
        self.cur_ms = now_ms;
        if self.since_ms == 0 {
            self.since_ms = now_ms;
        }

        // ---------------- CPU ----------------
        let load = 0.18 + 0.35 * noise("load", t) + 0.1 * (t / 9.0).sin().abs();
        let thread_use: Vec<f64> =
            (0..16).map(|i| (load * 100.0 * (0.4 + 1.3 * noise(&format!("th{i}"), t))).clamp(0.5, 100.0)).collect();
        let cpu_total = thread_use.iter().sum::<f64>() / 16.0;
        let cpu_max = thread_use.iter().cloned().fold(0.0, f64::max);
        let core_clk: Vec<f64> = (0..8).map(|i| 3900.0 + 1100.0 * noise(&format!("clk{i}"), t)).collect();
        let clk_avg = core_clk.iter().sum::<f64>() / 8.0;
        let tctl = 40.0 + 24.0 * noise("tl", t / 8.0) + 2.0 * noise("tctl", t / 3.0) + if cpu_thermal_on(t) { 24.0 } else { 0.0 };
        let ccd = tctl - 3.0;
        let pkg_w = 28.0 + cpu_total * 1.3 + 6.0 * noise("pkg", t);

        let mut cpu_sections = Vec::new();
        let rows = vec![
            rd("cpu/usage/total", "Total CPU Usage", Unit::Percent, cpu_total, None, None, Some("All logical CPUs, from /proc/stat")),
            rd("cpu/usage/max", "Max Thread Usage", Unit::Percent, cpu_max, None, None, None),
        ];
        let mut rows = rows;
        for i in 0..8 {
            let core = (thread_use[2 * i] + thread_use[2 * i + 1]) / 2.0;
            rows.push(rd(&format!("cpu/usage/core{i}"), &format!("Core {i} Usage"), Unit::Percent, core, None, None, None));
        }
        cpu_sections.push(self.section("cpu/usage", "Usage", false, rows));
        let mut rows = vec![rd("cpu/clock/avg", "Average Effective Clock", Unit::Mhz, clk_avg, None, None, Some("Mean of the per-core effective clocks"))];
        for (i, c) in core_clk.iter().enumerate() {
            rows.push(rd(&format!("cpu/clock/core{i}"), &format!("Core {i} Clock"), Unit::Mhz, *c, None, None, None));
        }
        cpu_sections.push(self.section("cpu/clock", "Clocks", false, rows));
        let rows = vec![
            rd("cpu/temp/tctl", "Tctl", Unit::Celsius, tctl, Some(80.0), Some(89.0), Some("k10temp Tctl")),
            rd("cpu/temp/ccd1", "Tccd1", Unit::Celsius, ccd, Some(80.0), Some(89.0), Some("k10temp Tccd1")),
        ];
        cpu_sections.push(self.section("cpu/temp", "Temperatures", false, rows));
        let rows = vec![
            rd("cpu/power/package", "CPU Package Power", Unit::Watt, pkg_w, None, None, Some("RAPL package energy, via the root helper")),
            rd("cpu/power/cores", "CPU Core Power", Unit::Watt, pkg_w * 0.72, None, None, None),
            rd("cpu/power/soc", "SoC Power", Unit::Watt, 11.0 + 2.0 * noise("soc", t), None, None, None),
            rd("cpu/volt/vcore", "Vcore", Unit::Volt, 1.05 + 0.25 * noise("vcore", t), None, None, None),
            rd("cpu/volt/vsoc", "VSoC", Unit::Volt, 1.2, None, None, None),
        ];
        cpu_sections.push(self.section("cpu/power", "Power and Voltage", false, rows));
        let mut rows = Vec::new();
        for (i, u) in thread_use.iter().enumerate() {
            rows.push(rd(&format!("cpu/thread/{i}/usage"), &format!("Thread {i} Usage"), Unit::Percent, *u, None, None, None));
        }
        for i in 0..16 {
            rows.push(rd(&format!("cpu/thread/{i}/clock"), &format!("Thread {i} Clock"), Unit::Mhz, core_clk[i / 2], None, None, None));
        }
        let lim = self.limits_section(
            "cpu/limits",
            vec![(
                "cpu/throttle/thermal",
                "Thermal throttling",
                cpu_thermal_on(t),
                "Yes while the CPU is cutting its clocks to stay under its temperature limit.",
            )],
        );
        cpu_sections.push(lim);
        cpu_sections.push(self.section("cpu/thread", "Per thread", true, rows));
        let cpu = SensorGroup {
            id: "cpu".into(),
            title: "CPU".into(),
            subtitle: Some("AMD Ryzen 7 7800X3D".into()),
            sections: cpu_sections,
        };

        // ---------------- GPU ----------------
        let gload = 0.25 + 0.6 * noise("gload", t);
        let gpu_use = gload * 100.0;
        let gslow = 0.25 + 0.6 * noise("gslow", t / 8.0);
        let gpu_t = 35.0 + gslow * 32.0 + 2.0 * noise("gt", t / 8.0);
        let hot = gpu_t + 9.0 + 3.0 * noise("hot", t / 8.0) + if gpu_thermal_on(t) { 12.0 } else { 0.0 };
        let vram_t = gpu_t + 14.0 + 4.0 * noise("vt", t / 8.0);
        let vram_used = (6.0 + 9.0 * noise("vram", t / 3.0)) * GIB;
        let vram_total = 24.0 * GIB;
        let gpu_w = 40.0 + gload * 300.0;
        let mut gpu_sections = Vec::new();
        let rows = vec![
            rd("gpu0/load/core", "GPU Core Usage", Unit::Percent, gpu_use, None, None, Some("NVML utilization.gpu")),
            rd("gpu0/load/memctl", "Memory Controller Usage", Unit::Percent, gpu_use * 0.45, None, None, None),
            rd("gpu0/load/enc", "Video Encode", Unit::Percent, 12.0 * noise("enc", t), None, None, None),
            rd("gpu0/load/dec", "Video Decode", Unit::Percent, 5.0 * noise("dec", t), None, None, None),
        ];
        gpu_sections.push(self.section("gpu0/load", "Usage", false, rows));
        let rows = vec![
            rd("gpu0/temp/core", "GPU Temperature", Unit::Celsius, gpu_t, Some(80.0), Some(90.0), None),
            rd("gpu0/temp/hotspot", "GPU Hotspot", Unit::Celsius, hot, Some(90.0), Some(100.0), Some("LACT hotspot sensor")),
            rd("gpu0/temp/vram", "VRAM Temperature", Unit::Celsius, vram_t, Some(95.0), Some(105.0), Some("Hottest memory chip")),
        ];
        gpu_sections.push(self.section("gpu0/temp", "Temperatures", false, rows));
        let rows = vec![
            rd("gpu0/clock/gfx", "Graphics Clock", Unit::Mhz, 300.0 + gload * 2500.0, None, None, None),
            rd("gpu0/clock/mem", "Memory Clock", Unit::Mhz, 10501.0, None, None, None),
            rd("gpu0/power/board", "Board Power", Unit::Watt, gpu_w, Some(400.0), Some(450.0), None),
            rd("gpu0/power/limit", "Power Limit Used", Unit::Percent, gpu_w / 450.0 * 100.0, None, None, None),
        ];
        gpu_sections.push(self.section("gpu0/clocks", "Clocks and Power", false, rows));
        let rows = vec![
            rd("gpu0/fan/1", "Fan 1", Unit::Rpm, 900.0 + gload * 1200.0, None, None, None),
            rd("gpu0/fan/2", "Fan 2", Unit::Rpm, 880.0 + gload * 1180.0, None, None, None),
            rd("gpu0/fan/pct", "Fan Speed", Unit::Percent, 30.0 + gload * 40.0, None, None, None),
        ];
        gpu_sections.push(self.section("gpu0/fan", "Fans", false, rows));
        let rows = vec![
            rd("gpu0/mem/used", "VRAM Used", Unit::Bytes, vram_used, None, None, None),
            rd("gpu0/mem/free", "VRAM Free", Unit::Bytes, vram_total - vram_used, None, None, None),
            rd("gpu0/mem/total", "VRAM Total", Unit::Bytes, vram_total, None, None, None),
            rd("gpu0/mem/pct", "VRAM Used", Unit::Percent, vram_used / vram_total * 100.0, Some(90.0), Some(97.0), None),
        ];
        gpu_sections.push(self.section("gpu0/mem", "Memory", false, rows));
        let mut rows = Vec::new();
        for i in 0..12 {
            rows.push(rd(&format!("gpu0/chip/{i}"), &format!("VRAM Chip {i}"), Unit::Celsius, vram_t - 3.0 + 6.0 * noise(&format!("chip{i}"), t), Some(95.0), Some(105.0), None));
        }
        gpu_sections.push(self.section("gpu0/chips", "VRAM chips", true, rows));
        let lim = self.limits_section(
            "gpu0/limits",
            vec![
                (
                    "gpu0/throttle/thermal",
                    "Thermal throttling",
                    gpu_thermal_on(t),
                    "Yes while the GPU is lowering its clocks because it is too hot.",
                ),
                (
                    "gpu0/throttle/power",
                    "Power throttling",
                    gpu_power_on(t),
                    "Yes while the GPU is held back by its power limit.",
                ),
            ],
        );
        gpu_sections.push(lim);
        let gpu = SensorGroup {
            id: "gpu0".into(),
            title: "GPU".into(),
            subtitle: Some("NVIDIA GeForce RTX 4090 · 24 GB".into()),
            sections: gpu_sections,
        };

        // ---------------- Memory ----------------
        let ram_total = 64.0 * GIB;
        let ram_used = (21.0 + 6.0 * noise("ram", t / 2.0)) * GIB;
        let mut mem_sections = Vec::new();
        let rows = vec![
            rd("memory/usage/used", "Memory Used", Unit::Bytes, ram_used, None, None, None),
            rd("memory/usage/available", "Memory Available", Unit::Bytes, ram_total - ram_used, None, None, None),
            rd("memory/usage/pct", "Memory Usage", Unit::Percent, ram_used / ram_total * 100.0, Some(85.0), Some(95.0), None),
            rd("memory/usage/cache", "Page Cache", Unit::Bytes, 18.0 * GIB + noise("cache", t) * GIB, None, None, None),
            rd("memory/usage/swap", "Swap Used", Unit::Bytes, 0.3 * GIB, None, None, None),
        ];
        mem_sections.push(self.section("memory/usage", "Usage", false, rows));
        let rows = vec![
            rd("memory/speed/configured", "Configured Speed", Unit::Mts, 6000.0, None, None, Some("SMBIOS type 17, via the root helper")),
            rd("memory/speed/voltage", "Configured Voltage", Unit::Volt, 1.35, None, None, None),
        ];
        mem_sections.push(self.section("memory/speed", "Configuration", false, rows));
        let mut rows = Vec::new();
        for (i, slot) in ["A1", "A2", "B1", "B2"].iter().enumerate() {
            rows.push(rd(&format!("memory/temp/dimm{}", i + 1), &format!("DIMM {slot} Temperature"), Unit::Celsius, 36.0 + 8.0 * noise(&format!("dimm{i}"), t / 8.0), Some(55.0), Some(85.0), None));
        }
        mem_sections.push(self.section("memory/temp", "Temperatures", false, rows));
        let memory = SensorGroup {
            id: "memory".into(),
            title: "Memory".into(),
            subtitle: Some("64 GB DDR5-6000 · 4 x G.Skill F5-6000J3038F16G".into()),
            sections: mem_sections,
        };

        // ---------------- Drives ----------------
        let mut groups = vec![cpu, gpu, memory];
        // (id, model, short mount, used share, life remaining)
        let drives = [
            ("nvme0", "Samsung SSD 990 PRO 2TB", "/", 62.0, 0.9),
            ("nvme1", "WD_BLACK SN850X 4TB", "files", 48.0, 0.55),
            ("nvme2", "Crucial T500 1TB", "games", 71.0, 0.35),
        ];
        for (n, (id, model, mount, used, life)) in drives.iter().enumerate() {
            let busy = noise(&format!("io{n}"), t * 1.3);
            let temp = 34.0 + 22.0 * busy + 4.0 * n as f64;
            let mut secs = Vec::new();
            let rows = vec![
                rd(&format!("{id}/temp/composite"), "Composite", Unit::Celsius, temp, Some(65.0), Some(80.0), None),
                rd(&format!("{id}/temp/s1"), "Controller", Unit::Celsius, temp + 6.0, Some(75.0), Some(90.0), None),
            ];
            secs.push(self.section(&format!("{id}/temp"), "Temperatures", false, rows));
            let rows = vec![
                rd(&format!("{id}/io/read"), "Read Rate", Unit::BytesPerSec, busy.powi(3) * 3.2e9, None, None, None),
                rd(&format!("{id}/io/write"), "Write Rate", Unit::BytesPerSec, busy.powi(4) * 1.4e9, None, None, None),
            ];
            secs.push(self.section(&format!("{id}/io"), "Activity", false, rows));
            let rows = vec![
                rd(&format!("{id}/cap/used"), "Capacity Used", Unit::Percent, *used, Some(90.0), Some(97.0), None),
                rd(&format!("{id}/cap/life"), "Life Remaining", Unit::Percent, life * 100.0, None, None, Some("NVMe SMART percentage used, inverted")),
            ];
            secs.push(self.section(&format!("{id}/cap"), "Capacity and health", false, rows));
            groups.push(SensorGroup {
                id: (*id).into(),
                title: format!("Drive {n}"),
                subtitle: Some(format!("{model} · {mount}")),
                sections: secs,
            });
        }

        // ---------------- Motherboard ----------------
        let mut secs = Vec::new();
        let rows = vec![
            rd("mb/temp/socket", "CPU Socket", Unit::Celsius, 40.0 + 6.0 * noise("sock", t), None, None, None),
            rd("mb/temp/vrm", "VRM MOS", Unit::Celsius, 45.0 + 10.0 * load, Some(95.0), Some(110.0), None),
            rd("mb/temp/chipset", "Chipset", Unit::Celsius, 52.0 + 2.0 * noise("chip", t), Some(85.0), Some(100.0), None),
            rd("mb/temp/ambient", "Motherboard", Unit::Celsius, 33.0 + 2.0 * noise("amb", t / 8.0), None, None, None),
        ];
        secs.push(self.section("mb/temp", "Temperatures", false, rows));
        let rows = vec![
            rd("mb/fan/cpu", "CPU Fan", Unit::Rpm, 900.0 + 800.0 * load, None, None, None),
            rd("mb/fan/pump", "AIO Pump", Unit::Rpm, 2650.0, None, None, None),
            rd("mb/fan/ch1", "Chassis Fan 1", Unit::Rpm, 780.0 + 200.0 * noise("c1", t), None, None, None),
            rd("mb/fan/ch2", "Chassis Fan 2", Unit::Rpm, 790.0 + 200.0 * noise("c2", t), None, None, None),
            rd("mb/fan/ch3", "Chassis Fan 3", Unit::Rpm, 1010.0, None, None, None),
        ];
        secs.push(self.section("mb/fan", "Fans", false, rows));
        let rows = vec![
            rd("mb/volt/12v", "+12V", Unit::Volt, 12.096, None, None, None),
            rd("mb/volt/5v", "+5V", Unit::Volt, 5.040, None, None, None),
            rd("mb/volt/3v3", "+3.3V", Unit::Volt, 3.312, None, None, None),
            rd("mb/volt/dram", "DRAM", Unit::Volt, 1.352, None, None, None),
        ];
        secs.push(self.section("mb/volt", "Voltages", false, rows));
        let rows = vec![rd("mb/misc/pcie", "PCIe Slot Power", Unit::Watt, 4.0 + 20.0 * gload, None, None, None)];
        secs.push(self.section("mb/misc", "Other", false, rows));
        groups.push(SensorGroup {
            id: "mb".into(),
            title: "Motherboard".into(),
            subtitle: Some("ASUS ROG STRIX B650E-E GAMING WIFI".into()),
            sections: secs,
        });

        // ---------------- Integrated GPU (collapsed) ----------------
        let mut secs = Vec::new();
        let rows = vec![
            rd("igpu/usage/gfx", "GFX Usage", Unit::Percent, 2.0 * noise("ig", t), None, None, None),
            rd("igpu/clock/gfx", "GFX Clock", Unit::Mhz, 400.0, None, None, None),
        ];
        secs.push(self.section("igpu/usage", "Usage", true, rows));
        let rows = vec![rd("igpu/temp/edge", "Edge Temperature", Unit::Celsius, 39.0 + noise("ige", t), None, None, None)];
        secs.push(self.section("igpu/temp", "Temperatures", true, rows));
        groups.push(SensorGroup {
            id: "igpu".into(),
            title: "Integrated GPU".into(),
            subtitle: Some("AMD Radeon Graphics (Raphael)".into()),
            sections: secs,
        });

        groups.iter_mut().for_each(arrange);
        let procs = self.processes(t, load, cpu_total as f32);
        let summary = Summary {
            unix_ms: now_ms,
            cpu_usage_pct: Some(cpu_total),
            cpu_temp_c: Some(tctl),
            cpu_power_w: Some(pkg_w),
            cpu_clock_avg_mhz: Some(clk_avg),
            gpu_usage_pct: Some(gpu_use),
            gpu_temp_c: Some(gpu_t),
            gpu_hotspot_c: Some(hot),
            gpu_vram_temp_c: Some(vram_t),
            gpu_power_w: Some(gpu_w),
            gpu_vram_used_bytes: Some(vram_used),
            gpu_vram_total_bytes: Some(vram_total),
            ram_used_pct: Some(ram_used / ram_total * 100.0),
            ram_used_bytes: Some(ram_used),
            ram_total_bytes: Some(ram_total),
            ram_temp_max_c: Some(44.0),
            ssd_temp_max_c: Some(55.0),
            disk_read_bps: Some(2.1e8),
            disk_write_bps: Some(3.0e7),
            swap_total_bytes: Some(8.0 * GIB),
        };
        Snapshot {
            proto: PROTO_VERSION,
            seq: self.seq,
            unix_ms: now_ms,
            stats_since_unix_ms: self.since_ms,
            interval_ms: 1000,
            history_minutes: 30,
            hostname: "desktop".into(),
            sensors: groups,
            sources: vec![
                SourceStatus { name: "nvml".into(), ok: true, detail: "NVML 595.58, 1 device".into() },
                SourceStatus { name: "lact".into(), ok: true, detail: "LACT daemon socket answered".into() },
                SourceStatus { name: "helper".into(), ok: true, detail: "hwglance-helper: RAPL and SMBIOS available".into() },
                SourceStatus { name: "hyprland".into(), ok: true, detail: "Window list from hyprctl clients".into() },
            ],
            procs: Some(procs),
            summary,
            crossings: self.crossings.list().to_vec(),
            limit_overrides: std::mem::take(&mut self.shown),
            limits_error: None,
        }
    }

    fn processes(&self, t: f64, load: f64, machine_pct: f32) -> Vec<ProcGroup> {
        // (id, class, display name, process names, base cpu, base mem MiB, gpu, window title)
        struct Spec(&'static str, ProcClass, &'static str, &'static [&'static str], f64, f64, Option<f64>, Option<&'static str>, Option<&'static str>);
        use ProcClass::*;
        let specs: Vec<Spec> = vec![
            Spec("app:brave-browser", App, "Brave", &["brave", "brave", "brave --type=gpu-process", "brave --type=renderer", "brave --type=renderer", "brave --type=renderer", "brave --type=renderer", "brave --type=utility", "brave --type=utility", "brave --type=zygote"], 6.0, 2600.0, Some(8.0), Some("Inbox - Brave"), Some("brave-browser")),
            Spec("app:steam", App, "Steam", &["steam", "steamwebhelper", "steamwebhelper", "steamwebhelper", "steamwebhelper", "pressure-vessel"], 1.5, 1400.0, Some(1.0), Some("Library"), Some("steam")),
            Spec("app:game", App, "Elden Ring", &["eldenring.exe", "wineserver", "services.exe", "explorer.exe", "winedevice.exe"], 38.0, 9800.0, Some(78.0), Some("ELDEN RING"), None),
            Spec("app:foot", App, "foot", &["foot", "fish", "btop"], 0.6, 90.0, None, Some("btop"), Some("foot")),
            Spec("app:code", App, "Code", &["code", "code", "code", "code", "code", "code"], 3.0, 1800.0, None, Some("hwglance - Visual Studio Code"), Some("code")),
            Spec("app:zed", App, "Zed", &["zed-editor", "zed-editor", "zed-editor", "zed-editor"], 0.8, 700.0, None, Some("main.rs - hwglance - Zed"), Some("zed")),
            Spec("app:discord", App, "Discord", &["Discord", "Discord", "Discord", "Discord"], 1.8, 950.0, Some(2.0), Some("#general - Discord"), Some("discord")),
            Spec("app:nautilus", App, "Files", &["nautilus"], 0.1, 140.0, None, Some("Documents"), Some("org.gnome.Nautilus")),
            Spec("app:mpv", App, "mpv", &["mpv"], 4.0, 380.0, Some(6.0), Some("movie.mkv - mpv"), Some("mpv")),
            Spec("app:telegram", App, "Telegram", &["telegram-desktop", "telegram-desktop", "telegram-desktop"], 1.0, 600.0, None, Some("Telegram"), None),
            Spec("unit:speech-dispatcher.service", Background, "speech-dispatcher", &["speech-dispatcher"], 0.2, 480.0, Some(0.0), None, None),
            Spec("unit:pipewire.service", Background, "pipewire", &["pipewire", "pipewire-pulse", "wireplumber"], 0.9, 60.0, None, None, None),
            Spec("unit:hyprland", Background, "Hyprland", &["Hyprland", "start-hyprland"], 1.4, 210.0, Some(1.0), None, None),
            Spec("unit:quickshell", Background, "quickshell", &["quickshell"], 0.5, 260.0, None, None, None),
            Spec("unit:xwayland", Background, "Xwayland", &["Xwayland"], 0.3, 120.0, None, None, None),
            Spec("unit:hwglanced.service", Background, "hwglanced", &["hwglanced"], 0.6, 24.0, None, None, None),
            Spec("unit:dbus", Background, "dbus-broker", &["dbus-broker-launch", "dbus-broker"], 0.0, 6.0, None, None, None),
            Spec("unit:syncthing", Background, "syncthing", &["syncthing", "syncthing"], 0.2, 210.0, None, None, None),
            Spec("unit:ssh-agent", Background, "ssh-agent", &["ssh-agent"], 0.0, 2.0, None, None, None),
            Spec("unit:mako", Background, "mako", &["mako"], 0.0, 18.0, None, None, None),
            Spec("unit:waybar", Background, "waybar", &["waybar"], 0.2, 70.0, None, None, None),
            Spec("unit:gvfs", Background, "gvfsd", &["gvfsd", "gvfsd-fuse", "gvfsd-trash", "gvfs-udisks2-volume-monitor"], 0.0, 30.0, None, None, None),
            Spec("unit:systemd-user", Background, "systemd (user)", &["systemd", "(sd-pam)"], 0.0, 12.0, None, None, None),
            Spec("unit:systemd", System, "systemd", &["systemd"], 0.0, 18.0, None, None, None),
            Spec("unit:systemd-journald", System, "systemd-journald", &["systemd-journald"], 0.1, 90.0, None, None, None),
            Spec("unit:systemd-udevd", System, "systemd-udevd", &["systemd-udevd"], 0.0, 20.0, None, None, None),
            Spec("unit:systemd-logind", System, "systemd-logind", &["systemd-logind"], 0.0, 9.0, None, None, None),
            Spec("unit:NetworkManager", System, "NetworkManager", &["NetworkManager"], 0.1, 28.0, None, None, None),
            Spec("unit:docker", System, "dockerd", &["dockerd", "containerd", "docker-proxy", "docker-proxy"], 0.2, 240.0, None, None, None),
            Spec("unit:sshd", System, "sshd", &["sshd"], 0.0, 8.0, None, None, None),
            Spec("unit:cups", System, "cupsd", &["cupsd", "cups-browsed"], 0.0, 14.0, None, None, None),
            Spec("unit:nvidia-persistenced", System, "nvidia-persistenced", &["nvidia-persistenc"], 0.0, 3.0, None, None, None),
            Spec("unit:lact", System, "lact", &["lact"], 0.3, 40.0, None, None, None),
            Spec("unit:bluetoothd", System, "bluetoothd", &["bluetoothd"], 0.0, 7.0, None, None, None),
            Spec("unit:polkit", System, "polkitd", &["polkitd"], 0.0, 22.0, None, None, None),
            Spec("unit:avahi", System, "avahi-daemon", &["avahi-daemon", "avahi-daemon"], 0.0, 5.0, None, None, None),
            Spec("unit:upowerd", System, "upowerd", &["upowerd"], 0.0, 9.0, None, None, None),
        ];
        // Fill up to about 100 groups with small single-process daemons and user tools.
        let filler: Vec<(String, ProcClass)> = (0..70)
            .map(|i| {
                let names = ["tracker-miner", "gnome-keyring", "at-spi2", "xdg-portal", "xdg-document", "dconf", "pcscd", "wpa_supplicant", "chronyd", "rtkit", "thermald", "udisksd", "fwupd", "smartd", "irqbalance", "cronie", "tailscaled", "rsyslogd", "snapd", "geoclue"];
                let n = names[i % names.len()];
                let class = if i % 3 == 0 { Background } else { System };
                (format!("{n}-{}", i / names.len() + 1), class)
            })
            .collect();

        let kthread_names: Vec<String> = (0..300)
            .map(|i| match i % 6 {
                0 => format!("kworker/{}:{}", i % 16, i / 16),
                1 => format!("ksoftirqd/{}", i % 16),
                2 => format!("migration/{}", i % 16),
                3 => format!("irq/{}-xhci", 20 + i),
                4 => format!("rcu_task_{i}"),
                _ => format!("kthread_{i}"),
            })
            .collect();
        let mut pid = 700;
        let mut out = Vec::new();
        let tick = (t / 3.0).floor();
        let mk = |pid: &mut i32, g: &str, name: &str, class: ProcClass, cpu: f64, mem_mib: f64, gpu: Option<f64>, k: usize, t: f64| {
            *pid += 1 + (hash(name, *pid as u64) * 14.0) as i32;
            let uid = if class == System { 0 } else { 1000 };
            let key = format!("{g}{name}{k}");
            let c = (cpu * (0.4 + 1.2 * noise(&key, t))).max(0.0);
            let gpu_pct = gpu.map(|v| (v * (0.5 + noise(&key, t + 3.0))).min(99.0));
            let mem = (mem_mib * (0.9 + 0.2 * noise(&key, t / 4.0)) * 1024.0 * 1024.0) as u64;
            // Storage activity is only visible for the user's own processes.
            let disk = (uid != 0).then(|| {
                let (r_peak, w_peak) = match g {
                    "app:game" => (190e6, 6e6),
                    "app:steam" => (8e6, 75e6),
                    "app:code" => (4e6, 2e6),
                    "app:brave-browser" => (3e6, 2.5e6),
                    _ => (0.25e6, 0.15e6),
                };
                let on = |salt: &str, p: f64| if noise(&format!("{key}{salt}"), t / 2.0) > p { noise(&format!("{key}{salt}v"), t) } else { 0.0 };
                ((on("r", 0.55) * r_peak) as f32, (on("w", 0.65) * w_peak) as f32)
            });
            // Apps started within the last day, daemons at boot or at login.
            let age_s = match class {
                App => 300.0 + 80_000.0 * hash(g, 5),
                Background => 90_000.0 + 20_000.0 * hash(g, 6),
                System => 200_000.0,
            };
            let started = self.start_unix_ms.saturating_sub((age_s * 1000.0) as u64 + k as u64 * 4000);
            ProcInfo {
                pid: *pid,
                ppid: 1,
                uid,
                name: name.split_whitespace().next().unwrap_or(name).chars().take(15).collect(),
                cmdline: format!("/usr/bin/{name} --demo-process"),
                state: 'S',
                threads: 1 + (hash(&key, 7) * 30.0) as u32,
                cpu_pct: c as f32,
                mem_bytes: mem,
                gpu_pct: gpu_pct.map(|v| v as f32),
                gpu_enc_pct: (g == "app:game" && k == 0).then(|| (4.0 * noise(&key, t)) as f32),
                gpu_dec_pct: match g {
                    "app:mpv" => Some((6.0 + 14.0 * noise(&key, t / 2.0)) as f32),
                    "app:brave-browser" if name.contains("gpu-process") => Some((3.0 * noise(&key, t)) as f32),
                    _ => None,
                },
                vram_bytes: gpu.map(|v| (v * 90.0 * 1024.0 * 1024.0 * (0.8 + 0.4 * noise(&key, t / 5.0))) as u64),
                start_time_ticks: *pid as u64 * 100,
                exe: demo_exe(g, name, class),
                disk_read_bps: disk.map(|d| d.0),
                disk_write_bps: disk.map(|d| d.1),
                started_unix_ms: started,
                user: if class == System { "root".into() } else { "alex".into() },
                critical: demo_critical(name),
            }
        };
        let mut group = |g: ProcGroupSpec| {
            let n = g.procs.len().max(1);
            let mut procs: Vec<ProcInfo> = g
                .procs
                .iter()
                .enumerate()
                .map(|(k, nm)| {
                    // The first process of a multi-process app carries the load of the main process.
                    let share = if k == 0 { 2.0 } else { 1.0 } / (n as f64 + 1.0);
                    mk(&mut pid, g.id, nm, g.class, g.cpu * share * n as f64 * (0.2 + load), g.mem * share * (n as f64 + 1.0) / n as f64 * 0.9, g.gpu.map(|v| v * share * (n as f64 + 1.0) / n as f64), k, t + tick)
                })
                .collect();
            // The first process is the parent of the rest, as in a real app.
            let root = procs.first().map(|p| p.pid).unwrap_or(1);
            for p in procs.iter_mut().skip(1) {
                p.ppid = root;
            }
            let sum_opt = |f: &dyn Fn(&ProcInfo) -> Option<f32>| -> Option<f32> {
                procs.iter().filter_map(f).fold(None, |a: Option<f32>, x| Some(a.unwrap_or(0.0) + x))
            };
            let disk_read_bps = sum_opt(&|p| p.disk_read_bps);
            let disk_write_bps = sum_opt(&|p| p.disk_write_bps);
            let gpu_enc_pct = sum_opt(&|p| p.gpu_enc_pct);
            let gpu_dec_pct = sum_opt(&|p| p.gpu_dec_pct);
            let cpu_pct: f32 = procs.iter().map(|p| p.cpu_pct).sum();
            let mem_bytes: u64 = procs.iter().map(|p| p.mem_bytes).sum();
            let gpu_sum: f32 = procs.iter().filter_map(|p| p.gpu_pct).sum();
            let vram_sum: u64 = procs.iter().filter_map(|p| p.vram_bytes).sum();
            let has_gpu = procs.iter().any(|p| p.gpu_pct.is_some());
            ProcGroup {
                id: g.id.into(),
                class: g.class,
                disk_read_bps,
                disk_write_bps,
                gpu_enc_pct,
                gpu_dec_pct,
                name: g.name.into(),
                icon: g.icon.map(Into::into),
                title: g.title.map(Into::into),
                procs,
                cpu_pct,
                mem_bytes,
                gpu_pct: has_gpu.then_some(gpu_sum),
                vram_bytes: has_gpu.then_some(vram_sum),
            }
        };
        for s in specs.iter() {
            out.push(group(ProcGroupSpec {
                id: s.0,
                class: s.1,
                name: s.2,
                procs: s.3.to_vec(),
                cpu: s.4,
                mem: s.5,
                gpu: s.6,
                title: s.7,
                icon: s.8,
            }));
        }
        out.push(group(ProcGroupSpec {
            id: "kthreads",
            class: System,
            name: "Kernel threads",
            procs: kthread_names.iter().map(|s| s.as_str()).collect(),
            cpu: 0.002,
            mem: 0.0,
            gpu: None,
            title: None,
            icon: None,
        }));
        let ids: Vec<String> = filler.iter().map(|(n, _)| format!("unit:{n}")).collect();
        for (i, (n, class)) in filler.iter().enumerate() {
            let many: Vec<&str> = if i % 5 == 0 { vec![n.as_str(); 4] } else if i % 7 == 0 { vec![n.as_str(), n.as_str()] } else { vec![n.as_str()] };
            out.push(group(ProcGroupSpec {
                id: &ids[i],
                class: *class,
                name: n,
                procs: many,
                cpu: 0.05 + 0.2 * hash(n, 1),
                mem: 4.0 + 80.0 * hash(n, 2),
                gpu: None,
                title: None,
                icon: None,
            }));
        }
        cap_cpu(&mut out, machine_pct);
        out
    }
}

/// Process shares are shares of the whole machine, so the made-up ones must not add up to more
/// than the machine is using: scale them down together (keeping their proportions) when they do.
fn cap_cpu(groups: &mut [ProcGroup], machine_pct: f32) {
    let total: f32 = groups.iter().map(|g| g.cpu_pct).sum();
    if total <= machine_pct || total <= 0.0 {
        return;
    }
    let k = machine_pct.max(0.0) / total;
    for g in groups.iter_mut() {
        g.cpu_pct *= k;
        g.procs.iter_mut().for_each(|p| p.cpu_pct *= k);
    }
}

/// What the collector would mark as critical, for the demo's stand-in processes.
fn demo_critical(name: &str) -> Option<String> {
    let kind = match name.to_lowercase().as_str() {
        "hyprland" | "start-hyprland" => "session",
        "systemd" => "system",
        "xwayland" => "xwayland",
        "quickshell" => "shell",
        "pipewire" | "pipewire-pulse" | "wireplumber" => "audio",
        "hwglanced" => "collector",
        _ => return None,
    };
    Some(kind.to_string())
}

/// Where a demo process's executable is: the way a real install lays them out.
fn demo_exe(group: &str, name: &str, class: ProcClass) -> Option<String> {
    let first = name.split_whitespace().next().unwrap_or(name);
    let path = match (group, first) {
        ("kthreads", _) => return None,
        ("app:brave-browser", _) => "/usr/lib/brave-bin/brave".to_string(),
        ("app:steam", "steamwebhelper") => "/home/alex/.local/share/Steam/ubuntu12_64/steamwebhelper".to_string(),
        ("app:steam", "pressure-vessel") => "/usr/lib/pressure-vessel/from-host/bin/pressure-vessel-wrap".to_string(),
        ("app:steam", _) => "/usr/lib/steam/steam".to_string(),
        ("app:game", "eldenring.exe") => "/home/alex/.local/share/Steam/steamapps/common/ELDEN RING/Game/eldenring.exe".to_string(),
        ("app:game", n) => format!("/usr/lib/wine/x86_64-unix/{n}"),
        ("app:code", _) => "/opt/visual-studio-code/code".to_string(),
        ("app:zed", _) => "/usr/lib/zed/zed-editor".to_string(),
        ("app:discord", _) => "/opt/discord/Discord".to_string(),
        ("app:telegram", _) => "/usr/bin/telegram-desktop".to_string(),
        ("unit:hwglanced.service", _) => "/home/alex/.local/bin/hwglanced".to_string(),
        (_, n) if n.starts_with("systemd") || n == "(sd-pam)" => format!("/usr/lib/systemd/{}", n.trim_matches(|c| c == '(' || c == ')')),
        (_, n) if class == ProcClass::System => format!("/usr/bin/{n}"),
        (_, n) => format!("/usr/bin/{n}"),
    };
    Some(path)
}

struct ProcGroupSpec<'a> {
    id: &'a str,
    class: ProcClass,
    name: &'a str,
    procs: Vec<&'a str>,
    cpu: f64,
    mem: f64,
    gpu: Option<f64>,
    title: Option<&'a str>,
    icon: Option<&'a str>,
}

/// Run the demo generator at 1 Hz, publishing into the link. It starts with 30 minutes of
/// history already behind it, and answers history requests like the collector would.
pub fn spawn(ctx: egui::Context, link: crate::conn::Link) {
    std::thread::Builder::new()
        .name("demo".into())
        .spawn(move || {
            let mut demo = Demo::new();
            demo.warm_up(T_START - HISTORY_S, T_START, 5.0);
            let t0 = std::time::Instant::now();
            loop {
                if link.take_demo_reset() {
                    demo.reset_stats();
                }
                for msg in link.take_demo_limits() {
                    demo.apply_limit_request(&msg);
                }
                let snap = demo.snapshot(T_START + t0.elapsed().as_secs_f64());
                link.publish(&ctx, snap);
                // Answer requests now and again shortly after, so a request that races a
                // snapshot is not left waiting a whole second.
                for _ in 0..10 {
                    for (ids, seconds) in link.take_demo_history() {
                        link.publish_history(&ctx, demo.history_reply(&ids, seconds));
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            }
        })
        .expect("spawn demo thread");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_cpu_shares_fit_the_machine() {
        let mut d = Demo::new();
        for t in [0.0, 10.0, 37.0, 120.0, 1500.0] {
            let s = d.snapshot(t);
            let total = s.summary.cpu_usage_pct.unwrap() as f32;
            let groups = s.procs.unwrap();
            let sum: f32 = groups.iter().map(|g| g.cpu_pct).sum();
            assert!(sum <= total + 0.01, "t={t}: groups add up to {sum} but the machine uses {total}");
            for g in &groups {
                assert!(g.cpu_pct <= 100.0, "{} at {}", g.name, g.cpu_pct);
                let members: f32 = g.procs.iter().map(|p| p.cpu_pct).sum();
                assert!((members - g.cpu_pct).abs() < 0.01);
            }
        }
    }

    #[test]
    fn demo_has_realistic_shape() {
        let mut d = Demo::new();
        let s = d.snapshot(10.0);
        let visible: usize = s
            .sensors
            .iter()
            .flat_map(|g| g.sections.iter())
            .filter(|sec| !sec.collapsed_by_default)
            .map(|sec| sec.readings.len())
            .sum();
        assert!((70..130).contains(&visible), "visible rows {visible}");
        let procs = s.procs.unwrap();
        let n: usize = procs.iter().map(|g| g.procs.len()).sum();
        assert!(procs.len() >= 90 && n >= 150, "{} groups, {n} procs", procs.len());
        let mut ids: Vec<&str> = s.sensors.iter().flat_map(|g| g.sections.iter()).flat_map(|s| s.readings.iter()).map(|r| r.id.as_str()).collect();
        let total = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), total, "reading ids must be unique");
    }

    #[test]
    fn limits_sections_carry_flag_readings_that_sometimes_turn_on() {
        let mut d = Demo::new();
        d.warm_up(T_START - 1000.0, T_START, 5.0);
        let s = d.snapshot(T_START + 1.0);
        let thermal = s.sensors.iter().flat_map(|g| g.sections.iter()).flat_map(|x| x.readings.iter()).find(|r| r.id == "cpu/throttle/thermal").unwrap();
        assert_eq!(thermal.unit, Unit::Flag);
        let ev = thermal.events.unwrap();
        assert!(ev.count >= 2, "two heat windows fall in 1000 s of warm-up: {ev:?}");
        assert!(ev.last_unix_ms > 0);
        assert!(thermal.stats.unwrap().max >= 1.0);
        assert!(s.sensors.iter().flat_map(|g| g.sections.iter()).flat_map(|x| x.readings.iter()).any(|r| r.id == "gpu0/throttle/power"));
        assert_eq!(s.history_minutes, 30);
        // while the window is on, the flag reads yes and the temperature is high
        let mut d2 = Demo::new();
        let s2 = d2.snapshot(562.0);
        let r = s2.sensors.iter().flat_map(|g| g.sections.iter()).flat_map(|x| x.readings.iter()).find(|r| r.id == "cpu/throttle/thermal").unwrap();
        assert_eq!(r.value, Some(1.0));
        let tctl = s2.sensors.iter().flat_map(|g| g.sections.iter()).flat_map(|x| x.readings.iter()).find(|r| r.id == "cpu/temp/tctl").unwrap();
        assert!(tctl.value.unwrap() > 70.0);
    }

    #[test]
    fn history_replies_cover_the_requested_window() {
        let mut d = Demo::new();
        d.warm_up(T_START - 1800.0, T_START, 5.0);
        let ids = vec!["cpu/temp/tctl".to_string(), "nope".to_string()];
        let reply = d.history_reply(&ids, 600);
        assert_eq!(reply.len(), 1, "unknown ids are skipped");
        let pts = &reply[0].points;
        assert!(pts.len() >= 100 && pts.len() <= 130, "{}", pts.len());
        assert!(pts.windows(2).all(|w| w[0].0 < w[1].0), "oldest first");
        assert_eq!(reply[0].unit, Unit::Celsius);
        let all = d.history_reply(&["cpu/temp/tctl".to_string()], 3600);
        assert!(all[0].points.len() > pts.len());
    }

    #[test]
    fn stats_follow_values_and_reset() {
        let mut d = Demo::new();
        for i in 0..5 {
            d.snapshot(i as f64 * 7.0);
        }
        let s = d.snapshot(40.0);
        let r = &s.sensors[0].sections[0].readings[0];
        assert!(r.stats.unwrap().samples >= 6);
        d.reset_stats();
        let s = d.snapshot(41.0);
        assert_eq!(s.sensors[0].sections[0].readings[0].stats.unwrap().samples, 1);
    }
}
