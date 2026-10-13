//! Tooltip text for every reading and the value each section shows on its header while collapsed.

use hwglance_proto::{Reading, Section, SensorGroup, Unit};

/// Number after `prefix` in an id (`cpu/usage/core3` -> 3).
fn num_after<'a>(id: &'a str, prefix: &str) -> &'a str {
    id.strip_prefix(prefix).unwrap_or("")
}

/// What the descriptions need to know about this machine.
pub struct Ctx {
    /// Logical CPUs.
    pub threads: usize,
    /// The CPU's rated top clock.
    pub max_mhz: Option<f64>,
    /// Where the CPU lowers its clocks (configured, reported by the driver, or from AMD's table).
    pub cpu_limit_c: f64,
    /// The CPU reports throttling itself (Intel), instead of it being inferred from the temperature.
    pub cpu_throttle_reported: bool,
}

impl Default for Ctx {
    fn default() -> Self {
        Ctx { threads: 0, max_mhz: None, cpu_limit_c: 95.0, cpu_throttle_reported: false }
    }
}

/// `gpu1`, `gpu2`, ...: a discrete AMD or Intel card (`gpu0` is the NVIDIA card).
fn is_extra_gpu(card: &str) -> bool {
    card.strip_prefix("gpu")
        .is_some_and(|n| !n.is_empty() && n != "0" && n.bytes().all(|b| b.is_ascii_digit()))
}

/// Text for the readings of a discrete AMD or Intel GPU card (`gpu1/...`); the reading ids
/// after the card follow the NVIDIA card's.
fn desc_gpu_card(rest: &str) -> Option<String> {
    let s = |t: &str| Some(t.to_string());
    match rest {
        "load/core" => s("How busy the GPU is: the share of time it was running work."),
        "load/memctl" => s("How busy the VRAM bus is. Not how full VRAM is (that is VRAM used)."),
        "temp/core" => s("Edge temperature: the GPU's main sensor, usually a few degrees below Hotspot."),
        "temp/hotspot" => s(
            "Junction temperature: the hottest point on the GPU die, normally 10 to 20 \u{b0}C above Edge.",
        ),
        "temp/vram" => s("Temperature of the video memory."),
        "clock/core" => s("Current graphics (shader) clock."),
        "clock/mem" => s("Current video memory clock."),
        "mem/used" => s("Video memory currently allocated by all programs."),
        "mem/usage" => s("VRAM used as a share of the card's total VRAM."),
        "power/draw" => s("Power the whole card draws right now, as the driver reports it."),
        "power/limit" => s("The most power the card may draw (its power cap)."),
        "power/voltage" => s("GPU core voltage."),
        "fan/pct" => s("Fan duty as a share of maximum."),
        "fan/rpm" => s("Measured fan speed."),
        _ => None,
    }
}

fn desc_for(id: &str, label: &str, ctx: &Ctx) -> Option<String> {
    let s = |t: &str| Some(t.to_string());
    let last = id.rsplit('/').next().unwrap_or("");
    if let Some((card, rest)) = id.split_once('/')
        && is_extra_gpu(card)
    {
        return desc_gpu_card(rest);
    }
    let limit = ctx.cpu_limit_c;
    match id {
        "cpu/usage/total" => {
            return Some(match ctx.threads {
                0 => "Share of all threads busy over the last second.".to_string(),
                n => format!("Share of all {n} threads busy over the last second."),
            });
        }
        "cpu/clock/avg" => {
            return Some(format!(
                "Mean effective clock across all {}threads. Idle threads pull it down, so it reads lower than Highest.",
                if ctx.threads == 0 { String::new() } else { format!("{} ", ctx.threads) }
            ));
        }
        "cpu/clock/max" => {
            return Some(match ctx.max_mhz {
                Some(m) => format!(
                    "Fastest thread right now: the clock single-threaded work gets. This CPU is rated up to {:.2} GHz.",
                    m / 1000.0
                ),
                None => "Fastest thread right now: the clock single-threaded work gets.".to_string(),
            });
        }
        "cpu/temp/tctl" => {
            return Some(format!(
                "AMD's control temperature: what fan curves and throttling use. On recent Ryzen it equals the die temperature (some older models add an offset). The CPU lowers its clocks around {limit:.0} \u{b0}C."
            ));
        }
        "cpu/temp/package" => {
            return Some(format!(
                "Intel's package temperature: the hottest spot of the whole CPU. The CPU lowers its clocks around {limit:.0} \u{b0}C."
            ));
        }
        "cpu/temp/board_cpu" => {
            return s(
                "The motherboard's reading of the CPU through its embedded controller. Smoother and slower than the CPU's own sensor.",
            );
        }
        "cpu/temp/board_package" => {
            return s("The board's package temperature reading. Tracks the CPU's own sensor closely.");
        }
        "cpu/power/package" => {
            return s(
                "Power for the whole CPU package, from the CPU's own energy counter. An estimate by the CPU, not a wall-socket measurement.",
            );
        }
        "gpu0/load/core" => {
            return s("How busy the GPU is: the share of time it was running work.");
        }
        "gpu0/load/memctl" => {
            return s("How busy the VRAM bus is. Not how full VRAM is (that is VRAM used).");
        }
        "gpu0/load/enc" => {
            return s("NVENC video encoder load: recording, streaming and Sunshine use it.");
        }
        "gpu0/load/dec" => return s("NVDEC video decoder load: hardware video playback uses it."),
        "gpu0/temp/core" => {
            return s("The GPU's main temperature, the one fan curves follow.");
        }
        "gpu0/temp/hotspot" => {
            return s(
                "Hottest point on the GPU die, normally 10 to 20 \u{b0}C above Core. A gap well over 25 \u{b0}C can mean poor cooler contact.",
            );
        }
        "gpu0/temp/vram" => {
            return s(
                "Memory junction temperature: the hottest spot across the memory chips. GDDR6X memory slows itself down around 110 \u{b0}C.",
            );
        }
        "gpu0/throttle/thermal" => {
            return s(
                "NVIDIA lowers the GPU clock to protect it from heat. Yes means it is happening now.",
            );
        }
        "gpu0/throttle/power" => {
            return s(
                "The card is holding its clock down to stay within its power limit. Normal under heavy load, not a heat problem.",
            );
        }
        "cpu/throttle/thermal" => {
            return Some(if ctx.cpu_throttle_reported {
                "The CPU counts the times it lowered its clocks to stay cool. Yes means it did so since the last sample.".to_string()
            } else {
                format!(
                    "AMD doesn't report throttling to Linux. This shows Yes while the CPU temperature sits at its {limit:.0} \u{b0}C limit, where the CPU lowers its clocks to stay there. Set cpu_thermal_limit_c in config.toml if your CPU differs."
                )
            });
        }
        "gpu0/clock/core" => return s("Current graphics (shader) clock."),
        "gpu0/clock/mem" => {
            return s(
                "VRAM clock as NVIDIA reports it. The data rate is a multiple of it (GDDR6X: double, so 11001 MHz means 22 Gbps). HWiNFO on Windows shows a smaller base clock for the same speed.",
            );
        }
        "gpu0/clock/video" => return s("Clock of the video encode and decode engines."),
        "gpu0/mem/used" => return s("Video memory currently allocated by all programs."),
        "gpu0/mem/usage" => return s("VRAM used as a share of the card's total VRAM."),
        "gpu0/power/draw" => return s("Power the whole card draws right now."),
        "gpu0/power/limit" => return s("The most power the card may draw (its enforced power limit)."),
        "gpu0/power/voltage" => return s("GPU core voltage, from LACT."),
        "gpu0/fan/pct" => return s("Fan speed as a share of maximum, set by the card's fan curve."),
        "gpu0/fan/rpm" => return s("Measured fan speed."),
        "memory/usage/used" => {
            return s(
                "RAM in use by programs. Cache is not counted, since the system frees it on demand.",
            );
        }
        "memory/usage/available" => {
            return s("RAM programs can still get, including cache the system can drop.");
        }
        "memory/usage/pct" => return s("Used as a share of total RAM."),
        "memory/usage/cached" => {
            return s(
                "Files kept in RAM to speed things up. Freed automatically when programs need the memory.",
            );
        }
        "memory/usage/swap" => {
            return s("Memory moved to swap (with zram, compressed RAM). Some use is normal.");
        }
        "memory/speed/mts" => {
            return s(
                "Memory speed the BIOS configured (the XMP or EXPO profile, else the default), in transfers per second.",
            );
        }
        "memory/speed/clock" => {
            return s(
                "Real memory clock: half the transfer rate, because DDR moves data twice per clock.",
            );
        }
        "igpu/main/temp" => return s("Temperature of the processor's built-in graphics."),
        "igpu/main/usage" => {
            return s(
                "Usage of the built-in graphics. Near zero is normal while it drives no monitor.",
            );
        }
        "igpu/main/clock" => return s("Current clock of the processor's built-in graphics."),
        "board/temp/asusec/motherboard" => {
            return s("General board temperature from ASUS's embedded controller.");
        }
        "board/temp/asusec/vrm" => {
            return s("CPU power delivery (VRM) temperature. Fine well into the 90s \u{b0}C.");
        }
        _ => {}
    }
    if id.starts_with("board/temp/") && last == "systin" {
        return s("Board thermistor read by the Super I/O chip; another motherboard temperature.");
    }
    if id.starts_with("board/temp/") && last == "cputin" {
        return s(
            "Super I/O thermistor near the CPU socket. Reads well below the CPU's own sensor; use that for the CPU itself.",
        );
    }
    if let Some(n) = id.strip_prefix("cpu/usage/core") {
        return Some(format!(
            "Average load of the threads of core {n} (T0 and T1 when it runs two)."
        ));
    }
    if let Some(n) = id.strip_prefix("cpu/clock/core") {
        return Some(format!(
            "Effective clock of core {n} (the faster of its threads)."
        ));
    }
    if id.starts_with("cpu/thread/") {
        return if id.ends_with("/usage") {
            s(
                "Usage of one logical thread. T0 and T1 are the threads of the same physical core.",
            )
        } else {
            s("Effective clock of one logical thread, as the kernel reports it.")
        };
    }
    if id.starts_with("cpu/temp/tccd") {
        return s(
            "Temperature of one core die (CCD): its cores and their cache. Usually a little below Tctl, which follows the hottest spot.",
        );
    }
    if id.starts_with("gpu0/vramchip/") {
        return s("Temperature of one memory chip (names as LACT reports them).");
    }
    if id.starts_with("memory/temp/dimm") {
        let n = num_after(id, "memory/temp/dimm");
        return Some(format!(
            "Temperature sensor on memory stick {n} (its SPD hub). DDR5 starts slowing down around 85 \u{b0}C."
        ));
    }
    if id.starts_with("disk/") {
        return match (last, id.contains("/temp/")) {
            ("read", false) => s("Data read from this drive per second."),
            ("write", false) => s("Data written to this drive per second."),
            ("active", false) => {
                s("Share of the last second the drive was busy, like Task Manager's Active time.")
            }
            ("composite", true) => s("The drive's overall temperature, the one it throttles on."),
            (_, true) => s(
                "An extra sensor inside the drive. The maker decides what it measures (often the controller or the flash).",
            ),
            _ => None,
        };
    }
    if id.starts_with("board/superio/") {
        return s("Unlabeled Super I/O input. It may be a header with nothing attached.");
    }
    if id.starts_with("board/temp/") && id.ends_with("/temp1") {
        return s("Temperature of the Wi-Fi card.");
    }
    if id == "board/fan/asusec/cpu_opt" {
        return s("Speed of the fan or pump on the CPU_OPT header.");
    }
    if id.starts_with("board/fan/") {
        // Headers with no name of their own are shown as "Fan N".
        return if label.starts_with("Fan ") {
            s("Unlabeled fan header. A steady speed around 4200 RPM usually means a pump.")
        } else {
            s("Speed of the fan or fans on this motherboard header.")
        };
    }
    None
}

/// Fill `desc` on every reading that has text for it.
pub fn describe(groups: &mut [SensorGroup], ctx: &Ctx) {
    for r in groups
        .iter_mut()
        .flat_map(|g| &mut g.sections)
        .flat_map(|s| &mut s.readings)
    {
        if r.desc.is_none() {
            r.desc = desc_for(&r.id, &r.label, ctx);
        }
    }
}

fn find<'a>(s: &'a Section, id: &str) -> Option<&'a Reading> {
    s.readings.iter().find(|r| r.id == id)
}

/// A new reading for a section summary; its id is `<section id>/summary`.
fn derived(s: &Section, label: &str, unit: Unit, value: Option<f64>, desc: &str) -> Reading {
    Reading {
        id: format!("{}/summary", s.id),
        label: label.into(),
        unit,
        value,
        stats: None,
        note: None,
        desc: Some(desc.into()),
        events: None,
        warn: None,
        crit: None,
    }
}

fn max_of(s: &Section, keep: impl Fn(&Reading) -> bool) -> Option<f64> {
    s.readings
        .iter()
        .filter(|r| keep(r))
        .filter_map(|r| r.value)
        .fold(None, |m: Option<f64>, v| Some(m.map_or(v, |m| m.max(v))))
}

fn with_thr(mut r: Reading, warn: Option<f64>, crit: Option<f64>) -> Reading {
    r.warn = warn;
    r.crit = crit;
    r
}

/// The reading the collapsed header of `s` shows.
fn summary_for(s: &Section) -> Option<Reading> {
    if s.readings.is_empty() {
        return None;
    }
    // Thresholds of the section's first reading, for derived maxima of like values.
    let first_thr = (s.readings[0].warn, s.readings[0].crit);
    let copy = |id: &str| find(s, id).cloned();
    // Discrete AMD and Intel cards (gpu1, gpu2, ...) follow the NVIDIA card's reading ids.
    if let Some((card, rest)) = s.id.split_once('/')
        && is_extra_gpu(card)
    {
        let id = |tail: &str| format!("{card}/{tail}");
        return match rest {
            "load" => copy(&id("load/core")),
            "temp" => copy(&id("temp/hotspot")).or_else(|| copy(&id("temp/core"))),
            "clock" => copy(&id("clock/core")),
            "mem" => copy(&id("mem/used")),
            "power" => copy(&id("power/draw")),
            _ => None,
        };
    }
    match s.id.as_str() {
        "cpu/usage" => copy("cpu/usage/total"),
        "cpu/clock" => copy("cpu/clock/max"),
        "cpu/thread" => Some(derived(
            s,
            "Busiest thread",
            Unit::Percent,
            max_of(s, |r| r.id.ends_with("/usage")),
            "Usage of the busiest single thread. Near 100 % while Total is low means one thread is the bottleneck, which is common in games.",
        )),
        "cpu/temp" => copy("cpu/temp/tctl").or_else(|| copy("cpu/temp/package")),
        "cpu/power" => copy("cpu/power/package"),
        "cpu/limits" => copy("cpu/throttle/thermal"),
        "gpu0/limits" => copy("gpu0/throttle/thermal"),
        "gpu0/load" => copy("gpu0/load/core"),
        "gpu0/temp" => copy("gpu0/temp/hotspot").or_else(|| copy("gpu0/temp/core")),
        "gpu0/vramchips" => Some(with_thr(
            derived(
                s,
                "Hottest chip",
                Unit::Celsius,
                max_of(s, |_| true),
                "Temperature of the hottest memory chip.",
            ),
            first_thr.0,
            first_thr.1,
        )),
        "gpu0/clock" => copy("gpu0/clock/core"),
        "gpu0/mem" => copy("gpu0/mem/used"),
        "gpu0/power" => copy("gpu0/power/draw"),
        "memory/usage" => copy("memory/usage/used"),
        "memory/temp" => Some(with_thr(
            derived(
                s,
                "Hottest DIMM",
                Unit::Celsius,
                max_of(s, |_| true),
                "Temperature of the hottest memory stick.",
            ),
            first_thr.0,
            first_thr.1,
        )),
        "memory/speed" => copy("memory/speed/mts"),
        "board/temp" | "board/superio" => Some(derived(
            s,
            "Hottest",
            Unit::Celsius,
            max_of(s, |_| true),
            "The highest temperature in this list.",
        )),
        "board/fan" => Some(derived(
            s,
            "Fastest fan",
            Unit::Rpm,
            max_of(s, |_| true),
            "Speed of the fastest fan or pump on this motherboard.",
        )),
        "igpu/main" => copy("igpu/main/temp").or_else(|| copy("igpu/main/clock")),
        id if id.starts_with("disk/") && id.ends_with("/io") => {
            let get = |suffix: &str| {
                s.readings
                    .iter()
                    .find(|r| r.id.ends_with(suffix))
                    .and_then(|r| r.value)
            };
            let value = match (get("/read"), get("/write")) {
                (None, None) => None,
                (a, b) => Some(a.unwrap_or(0.0) + b.unwrap_or(0.0)),
            };
            Some(derived(
                s,
                "Read + write",
                Unit::BytesPerSec,
                value,
                "Total data moved to and from this drive per second.",
            ))
        }
        id if id.starts_with("disk/") && id.ends_with("/temp") => s
            .readings
            .iter()
            .find(|r| r.id.ends_with("/temp/composite"))
            .cloned()
            .or_else(|| {
                Some(with_thr(
                    derived(
                        s,
                        "Hottest sensor",
                        Unit::Celsius,
                        max_of(s, |_| true),
                        "The highest temperature reported by this drive.",
                    ),
                    first_thr.0,
                    first_thr.1,
                ))
            }),
        _ => None,
    }
}

pub fn add_summaries(groups: &mut [SensorGroup]) {
    for s in groups.iter_mut().flat_map(|g| &mut g.sections) {
        s.summary = summary_for(s);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(id: &str, v: f64) -> Reading {
        Reading {
            id: id.into(),
            label: id.into(),
            unit: Unit::Celsius,
            value: Some(v),
            stats: None,
            note: None,
            desc: None,
            events: None,
            warn: Some(90.0),
            crit: Some(100.0),
        }
    }

    fn section(id: &str, readings: Vec<Reading>) -> Section {
        Section {
            id: id.into(),
            title: id.into(),
            collapsed_by_default: false,
            readings,
            summary: None,
        }
    }

    #[test]
    fn copy_and_derived_summaries() {
        let s = section(
            "cpu/temp",
            vec![r("cpu/temp/tccd1", 60.0), r("cpu/temp/tctl", 70.0)],
        );
        assert_eq!(summary_for(&s).unwrap().id, "cpu/temp/tctl");
        let s = section(
            "gpu0/vramchips",
            vec![r("gpu0/vramchip/A0", 60.0), r("gpu0/vramchip/B0", 66.0)],
        );
        let sum = summary_for(&s).unwrap();
        assert_eq!(
            (sum.id.as_str(), sum.value, sum.warn),
            ("gpu0/vramchips/summary", Some(66.0), Some(90.0))
        );
        let s = section(
            "disk/X/temp",
            vec![
                r("disk/X/temp/sensor_1", 50.0),
                r("disk/X/temp/sensor_2", 55.0),
            ],
        );
        assert_eq!(summary_for(&s).unwrap().value, Some(55.0));
        assert!(summary_for(&section("board/fan", vec![])).is_none());
    }

    #[test]
    fn io_sum() {
        let mut a = r("disk/X/io/read", 100.0);
        a.unit = Unit::BytesPerSec;
        let mut b = r("disk/X/io/write", 50.0);
        b.unit = Unit::BytesPerSec;
        let sum = summary_for(&section(
            "disk/X/io",
            vec![a, b, r("disk/X/io/active", 3.0)],
        ))
        .unwrap();
        assert_eq!(
            (sum.id.as_str(), sum.value),
            ("disk/X/io/summary", Some(150.0))
        );
    }

    #[test]
    fn descriptions() {
        assert!(
            desc_for("cpu/usage/core3", "Core 3", &Ctx::default())
                .unwrap()
                .contains("core 3")
        );
        assert!(
            desc_for("memory/temp/dimm2", "DIMM 2", &Ctx::default())
                .unwrap()
                .contains("stick 2")
        );
        assert!(
            desc_for("board/fan/nct6799/fan7", "Fan 7", &Ctx::default())
                .unwrap()
                .contains("Unlabeled")
        );
        assert!(
            desc_for("board/fan/nct6799/fan1", "Top intake", &Ctx::default())
                .unwrap()
                .contains("this motherboard header")
        );
        assert!(
            desc_for("disk/S/temp/sensor_1", "Sensor 1", &Ctx::default())
                .unwrap()
                .contains("extra sensor")
        );
    }

    #[test]
    fn texts_use_the_detected_hardware() {
        let ctx = Ctx { threads: 12, max_mhz: Some(4950.0), cpu_limit_c: 100.0, cpu_throttle_reported: false };
        let d = |id: &str| desc_for(id, "x", &ctx).unwrap();
        assert!(d("cpu/usage/total").contains("all 12 threads"));
        assert!(d("cpu/clock/avg").contains("all 12 threads"));
        assert!(d("cpu/clock/max").contains("4.95 GHz"));
        assert!(d("cpu/throttle/thermal").contains("100 \u{b0}C limit"));
        assert!(d("cpu/temp/tctl").contains("around 100 \u{b0}C"));
        let reported = Ctx { cpu_throttle_reported: true, ..Default::default() };
        assert!(desc_for("cpu/throttle/thermal", "x", &reported).unwrap().contains("counts"));
        // nothing names a particular CPU, board or card
        let all = [
            "cpu/usage/total", "cpu/clock/avg", "cpu/clock/max", "cpu/temp/tctl", "cpu/temp/board_cpu",
            "cpu/throttle/thermal", "gpu0/mem/usage", "gpu0/temp/vram", "igpu/main/usage",
            "board/temp/nct6798/systin", "board/temp/nct6798/cputin", "board/temp/mt7925_phy0/temp1",
        ];
        for id in all {
            let text = d(id);
            for name in ["7800", "X3D", "5.05", "24 GB", "ASUS's", "Sunshine here"] {
                assert!(!text.contains(name), "{id}: {text}");
            }
        }
        assert!(d("board/temp/nct6798/systin").contains("Super I/O"));
        assert!(d("board/temp/mt7925_phy0/temp1").contains("Wi-Fi"));
        // the extra GPU cards
        assert!(d("gpu1/temp/hotspot").contains("Junction"));
        assert!(d("gpu2/power/limit").contains("power cap"));
        assert!(desc_for("gpu1/load/enc", "x", &ctx).is_none());
    }

    #[test]
    fn extra_gpu_cards_get_the_same_summaries() {
        let s = section(
            "gpu1/temp",
            vec![r("gpu1/temp/core", 60.0), r("gpu1/temp/hotspot", 72.0)],
        );
        assert_eq!(summary_for(&s).unwrap().id, "gpu1/temp/hotspot");
        let s = section("gpu1/power", vec![r("gpu1/power/draw", 100.0)]);
        assert_eq!(summary_for(&s).unwrap().id, "gpu1/power/draw");
        let s = section("cpu/temp", vec![r("cpu/temp/core0", 60.0), r("cpu/temp/package", 70.0)]);
        assert_eq!(summary_for(&s).unwrap().id, "cpu/temp/package");
    }
}
