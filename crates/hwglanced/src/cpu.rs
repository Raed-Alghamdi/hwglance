//! CPU usage from /proc/stat, clocks and topology from sysfs.

use crate::util::{read_num, read_trim};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

struct Thread {
    cpu: u32,
    core: u32,
    /// 0 for the lower-numbered logical CPU of a core.
    t: u32,
    freq: PathBuf,
}

#[derive(Clone, Debug)]
pub struct ThreadVal {
    pub core: u32,
    pub t: u32,
    pub usage: Option<f64>,
    pub mhz: Option<f64>,
}

#[derive(Clone, Debug, Default)]
pub struct CpuVal {
    pub model: String,
    /// AMD's published maximum operating temperature for this model, when it is in the table.
    pub model_limit_c: Option<f64>,
    /// The CPU's highest rated clock (cpuinfo_max_freq).
    pub max_mhz: Option<f64>,
    /// Thermal throttling the CPU itself reported since the previous sample (Intel's
    /// thermal_throttle counters); `None` when the kernel exposes no such counters.
    pub throttling: Option<bool>,
    pub total: Option<f64>,
    pub threads: Vec<ThreadVal>,
    /// Jiffies (all CPUs together) since the previous sample; the divisor for per-process shares.
    pub jiffies: u64,
}

#[derive(Clone, Copy)]
struct Times {
    idle: u64,
    total: u64,
}

/// The kernel's thermal throttle counters: how often and for how long a core or package was
/// slowed down by heat. Sums are compared between samples.
#[derive(Default)]
struct Throttle {
    files: Vec<PathBuf>,
    prev: Option<u64>,
}

impl Throttle {
    /// `Some(true)` when a counter rose since the last call.
    fn poll(&mut self) -> Option<bool> {
        if self.files.is_empty() {
            return None;
        }
        let sum: u64 = self.files.iter().filter_map(read_num).map(|v| v as u64).sum();
        let rose = self.prev.is_some_and(|p| sum > p);
        self.prev = Some(sum);
        Some(rose)
    }
}

/// Counter files under `cpu_root` (`/sys/devices/system/cpu`): per core the lowest-numbered
/// thread's `core_throttle_count` and `core_throttle_total_time_ms`, per package the same for
/// `package_*`, as the other threads of a core or package show the same numbers.
fn throttle_files(cpu_root: &Path) -> Vec<PathBuf> {
    let mut cpus: Vec<u32> = fs::read_dir(cpu_root)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().to_string_lossy().strip_prefix("cpu")?.parse().ok())
        .collect();
    cpus.sort();
    let (mut cores, mut packages) = (HashSet::new(), HashSet::new());
    let mut out = Vec::new();
    for cpu in cpus {
        let dir = cpu_root.join(format!("cpu{cpu}"));
        let tt = dir.join("thermal_throttle");
        let pkg = read_trim(dir.join("topology/physical_package_id")).unwrap_or_default();
        let core = read_trim(dir.join("topology/core_id")).unwrap_or_default();
        let mut add = |prefix: &str| {
            for suffix in ["count", "total_time_ms"] {
                let f = tt.join(format!("{prefix}_throttle_{suffix}"));
                if f.exists() {
                    out.push(f);
                }
            }
        };
        if tt.is_dir() {
            if cores.insert((pkg.clone(), core)) {
                add("core");
            }
            if packages.insert(pkg) {
                add("package");
            }
        }
    }
    out
}

/// AMD's published maximum operating temperature (Tjmax) of a Ryzen, from its model name.
///
/// Source: the "Max. Operating Temperature (Tjmax)" column of AMD's processor specifications,
/// https://www.amd.com/en/products/specifications/processors.html (read 2026-10-13), and the
/// product pages behind it, for example
/// https://www.amd.com/en/products/processors/desktops/ryzen/7000-series/amd-ryzen-7-7800x3d.html (89 C).
/// AMD gives one value per model; the rules below fold the models into families, with desktop
/// (suffix X, X3D, G, GE, F, none) and mobile (U, H, HS, HX, C) parts apart:
/// - Ryzen 1000 to 4000 desktop 95 (2700X 85, its PRO version 95); Ryzen 2000 mobile 95;
///   Ryzen 3000 mobile 105 (3250U and the PRO parts 95); Ryzen 4000 mobile 105
/// - Ryzen 5000 desktop 90 for 5500, 5600, 5600X3D, 5700X, 5700X3D, 5800X, 5800X3D, 5800XT,
///   5900X, 5900XT and 5950X, else 95; Ryzen 5000 mobile 105, but 95 for the 5x25 and 5x75 parts
/// - Ryzen 6000 mobile 95
/// - Ryzen 7000 desktop 89 for X3D, else 95; mobile 89 for HX3D, 100 for the 7x40 and 7x45
///   parts, else 95
/// - Ryzen 8000G and 9000 desktop 95; Ryzen 8000 and 9000 mobile (also HX3D), AI 300 and AI Max 100
/// - Ryzen 100 95; Ryzen 200, 400 and AI 400 100 (AI 400 desktop G and GE 95)
/// - Threadripper 1000 and 2000 68, PRO 3000 WX 90, all later 95
///
/// `None` for anything else (older Athlon, FX and APU parts, EPYC, non-AMD CPUs).
pub fn amd_tjmax(model: &str) -> Option<f64> {
    let upper = model.to_ascii_uppercase();
    if !upper.contains("RYZEN") {
        return None;
    }
    let words: Vec<&str> = upper.split_whitespace().collect();
    let model_no = words.iter().skip_while(|w| !w.starts_with("RYZEN")).skip(1).find(|w| {
        w.len() >= 3 && w.chars().next().is_some_and(|c| c.is_ascii_digit()) && w.chars().filter(char::is_ascii_digit).count() >= 3
    })?;
    let digits: String = model_no.chars().take_while(char::is_ascii_digit).collect();
    let suffix = &model_no[digits.len()..];
    let n: u32 = digits.parse().ok()?;
    let pro = words.contains(&"PRO");
    if upper.contains("THREADRIPPER") {
        return Some(match (n / 1000, suffix, pro) {
            (1 | 2, _, false) => 68.0,
            (3, "WX", true) => 90.0,
            _ => 95.0,
        });
    }
    // Mobile parts end in U, H (HS, HX, HX3D) or C; desktop parts in X, G, F, E, T or nothing.
    let mobile = matches!(suffix.chars().next(), Some('U' | 'H' | 'C'));
    let last2 = n % 100;
    let t = if digits.len() == 3 {
        // Ryzen 100 (95), 200, 400 and AI 300/400 (100); the AI 400 desktop parts end in G or GE
        if n < 200 || suffix.starts_with('G') { 95.0 } else { 100.0 }
    } else {
        match (n / 1000, mobile) {
            (1..=4, false) => if *model_no == "2700X" && !pro { 85.0 } else { 95.0 },
            (2, true) => 95.0,
            (3, true) => if pro || *model_no == "3250U" { 95.0 } else { 105.0 },
            (4, true) => 105.0,
            (5, false) => {
                const NINETY: &[&str] = &[
                    "5950X", "5900X", "5900XT", "5800XT", "5800X3D", "5800X", "5700X3D", "5700X",
                    "5600X3D", "5600", "5500X3D", "5500",
                ];
                if NINETY.contains(model_no) { 90.0 } else { 95.0 }
            }
            (5, true) => if last2 == 25 || last2 == 75 { 95.0 } else { 105.0 },
            (6, _) => 95.0,
            (7, false) => if suffix.contains("X3D") { 89.0 } else { 95.0 },
            (7, true) => {
                if suffix.contains("X3D") {
                    89.0
                } else if last2 == 40 || last2 == 45 {
                    100.0
                } else {
                    95.0
                }
            }
            (8 | 9, false) => 95.0,
            (8 | 9, true) => 100.0,
            _ => return None,
        }
    };
    Some(t)
}

pub struct CpuSampler {
    model: String,
    model_limit_c: Option<f64>,
    max_mhz: Option<f64>,
    throttle: Throttle,
    threads: Vec<Thread>,
    prev_all: Option<Times>,
    prev_cpu: HashMap<u32, Times>,
}

fn parse_times(rest: &str) -> Option<Times> {
    let v: Vec<u64> = rest
        .split_whitespace()
        .take(8)
        .filter_map(|x| x.parse().ok())
        .collect();
    if v.len() < 5 {
        return None;
    }
    Some(Times {
        idle: v[3] + v[4],
        total: v.iter().sum(),
    })
}

fn usage(prev: Option<Times>, now: Times) -> Option<f64> {
    let p = prev?;
    let dt = now.total.checked_sub(p.total)?;
    if dt == 0 {
        return None;
    }
    let di = now.idle.saturating_sub(p.idle);
    Some((100.0 * (1.0 - di as f64 / dt as f64)).clamp(0.0, 100.0))
}

impl CpuSampler {
    pub fn new() -> Self {
        let model = fs::read_to_string("/proc/cpuinfo")
            .ok()
            .and_then(|t| {
                t.lines()
                    .find(|l| l.starts_with("model name"))
                    .and_then(|l| l.split_once(':'))
                    .map(|(_, v)| v.trim().to_string())
            })
            .unwrap_or_else(|| "CPU".into());
        let mut raw: Vec<(u32, u32)> = Vec::new(); // (cpu, core)
        for e in fs::read_dir("/sys/devices/system/cpu")
            .into_iter()
            .flatten()
            .flatten()
        {
            let name = e.file_name().to_string_lossy().into_owned();
            let Some(cpu) = name.strip_prefix("cpu").and_then(|n| n.parse::<u32>().ok()) else {
                continue;
            };
            if let Some(core) =
                read_trim(e.path().join("topology/core_id")).and_then(|s| s.parse().ok())
            {
                raw.push((cpu, core));
            }
        }
        raw.sort_by_key(|&(cpu, core)| (core, cpu));
        let mut threads = Vec::new();
        let mut last_core = None;
        let mut t = 0;
        for (cpu, core) in raw {
            t = if last_core == Some(core) { t + 1 } else { 0 };
            last_core = Some(core);
            threads.push(Thread {
                cpu,
                core,
                t,
                freq: PathBuf::from(format!(
                    "/sys/devices/system/cpu/cpu{cpu}/cpufreq/scaling_cur_freq"
                )),
            });
        }
        let max_mhz = threads
            .iter()
            .filter_map(|th| read_num(format!("/sys/devices/system/cpu/cpu{}/cpufreq/cpuinfo_max_freq", th.cpu)))
            .fold(None, |m: Option<f64>, khz| Some(m.map_or(khz, |m| m.max(khz))))
            .map(|khz| khz / 1000.0);
        CpuSampler {
            model_limit_c: amd_tjmax(&model),
            max_mhz,
            throttle: Throttle {
                files: throttle_files(Path::new("/sys/devices/system/cpu")),
                prev: None,
            },
            model,
            threads,
            prev_all: None,
            prev_cpu: HashMap::new(),
        }
    }

    pub fn sample(&mut self) -> CpuVal {
        let text = fs::read_to_string("/proc/stat").unwrap_or_default();
        let mut all = None;
        let mut per: HashMap<u32, Times> = HashMap::new();
        for line in text.lines() {
            let Some(rest) = line.strip_prefix("cpu") else {
                break;
            };
            let (id, vals) = rest.split_once(' ').unwrap_or((rest, ""));
            let Some(times) = parse_times(vals) else {
                continue;
            };
            if id.is_empty() {
                all = Some(times);
            } else if let Ok(n) = id.parse::<u32>() {
                per.insert(n, times);
            }
        }
        let jiffies = match (self.prev_all, all) {
            (Some(p), Some(n)) => n.total.saturating_sub(p.total),
            _ => 0,
        };
        let total = all.and_then(|n| usage(self.prev_all, n));
        let threads = self
            .threads
            .iter()
            .map(|th| ThreadVal {
                core: th.core,
                t: th.t,
                usage: per
                    .get(&th.cpu)
                    .and_then(|n| usage(self.prev_cpu.get(&th.cpu).copied(), *n)),
                mhz: read_num(&th.freq).map(|khz| khz / 1000.0),
            })
            .collect();
        self.prev_all = all;
        self.prev_cpu = per;
        CpuVal {
            model: self.model.clone(),
            model_limit_c: self.model_limit_c,
            max_mhz: self.max_mhz,
            throttling: self.throttle.poll(),
            total,
            threads,
            jiffies,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amd_limits_follow_the_published_table() {
        let t = |m: &str| amd_tjmax(m);
        // the X3D parts throttle at 89, most other Ryzen 7000 and 9000 desktop parts at 95
        assert_eq!(t("AMD Ryzen 7 7800X3D 8-Core Processor"), Some(89.0));
        assert_eq!(t("AMD Ryzen 9 7950X3D 16-Core Processor"), Some(89.0));
        assert_eq!(t("AMD Ryzen 9 7950X 16-Core Processor"), Some(95.0));
        assert_eq!(t("AMD Ryzen 5 7600 6-Core Processor"), Some(95.0));
        assert_eq!(t("AMD Ryzen 7 9800X3D 8-Core Processor"), Some(95.0));
        assert_eq!(t("AMD Ryzen 9 9950X3D 16-Core Processor"), Some(95.0));
        // Ryzen 5000: a split by model
        assert_eq!(t("AMD Ryzen 7 5800X3D 8-Core Processor"), Some(90.0));
        assert_eq!(t("AMD Ryzen 9 5950X 16-Core Processor"), Some(90.0));
        assert_eq!(t("AMD Ryzen 5 5600X 6-Core Processor"), Some(95.0));
        assert_eq!(t("AMD Ryzen 5 5600 6-Core Processor"), Some(90.0));
        assert_eq!(t("AMD Ryzen 5 5600G with Radeon Graphics"), Some(95.0));
        assert_eq!(t("AMD Ryzen 7 5800H with Radeon Graphics"), Some(105.0));
        assert_eq!(t("AMD Ryzen 7 5825U with Radeon Graphics"), Some(95.0));
        assert_eq!(t("AMD Ryzen 7 3700X 8-Core Processor"), Some(95.0));
        assert_eq!(t("AMD Ryzen 7 2700X Eight-Core Processor"), Some(85.0));
        // mobile
        assert_eq!(t("AMD Ryzen 7 7840HS w/ Radeon 780M Graphics"), Some(100.0));
        assert_eq!(t("AMD Ryzen 7 7735HS with Radeon Graphics"), Some(95.0));
        assert_eq!(t("AMD Ryzen 9 7945HX3D 16-Core Processor"), Some(89.0));
        assert_eq!(t("AMD Ryzen 9 7945HX 16-Core Processor"), Some(100.0));
        assert_eq!(t("AMD Ryzen 7 6800H with Radeon Graphics"), Some(95.0));
        assert_eq!(t("AMD Ryzen 9 9955HX3D 16-Core Processor"), Some(100.0));
        assert_eq!(t("AMD Ryzen AI 9 HX 370 w/ Radeon 890M"), Some(100.0));
        assert_eq!(t("AMD Ryzen AI Max+ 395 w/ Radeon 8060S"), Some(100.0));
        assert_eq!(t("AMD Ryzen 5 8600G w/ Radeon 760M Graphics"), Some(95.0));
        assert_eq!(t("AMD Ryzen 7 PRO 7840U w/ Radeon 780M Graphics"), Some(100.0));
        assert_eq!(t("AMD Ryzen Threadripper 1950X 16-Core Processor"), Some(68.0));
        assert_eq!(t("AMD Ryzen Threadripper 3970X 32-Core Processor"), Some(95.0));
        assert_eq!(t("AMD Ryzen Threadripper PRO 3955WX 16-Cores"), Some(90.0));
        assert_eq!(t("AMD Ryzen Threadripper PRO 7995WX 96-Cores"), Some(95.0));
        // not in the table
        assert_eq!(t("Intel(R) Core(TM) i7-12700K"), None);
        assert_eq!(t("AMD EPYC 7763 64-Core Processor"), None);
        assert_eq!(t("AMD Athlon 3000G with Radeon Graphics"), None);
        assert_eq!(t("CPU"), None);
    }

    fn write(p: &Path, text: &str) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, text).unwrap();
    }

    #[test]
    fn intel_throttle_counters_are_read_once_per_core_and_package() {
        let root = std::env::temp_dir().join(format!("hwglance-throttle-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        // two cores with two threads each, one package
        for (cpu, core) in [(0, 0), (1, 1), (2, 0), (3, 1)] {
            let d = root.join(format!("cpu{cpu}"));
            write(&d.join("topology/core_id"), &format!("{core}\n"));
            write(&d.join("topology/physical_package_id"), "0\n");
            write(&d.join("thermal_throttle/core_throttle_count"), "0\n");
            write(&d.join("thermal_throttle/package_throttle_count"), "0\n");
        }
        write(&root.join("cpu0/thermal_throttle/package_throttle_total_time_ms"), "0\n");
        write(&root.join("cpufreq/policy0"), ""); // not a cpu directory
        let files = throttle_files(&root);
        let names: Vec<String> = files
            .iter()
            .map(|f| f.strip_prefix(&root).unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            names,
            [
                "cpu0/thermal_throttle/core_throttle_count",
                "cpu0/thermal_throttle/package_throttle_count",
                "cpu0/thermal_throttle/package_throttle_total_time_ms",
                "cpu1/thermal_throttle/core_throttle_count",
            ]
        );
        let mut th = Throttle { files, prev: None };
        assert_eq!(th.poll(), Some(false), "the first call has nothing to compare with");
        assert_eq!(th.poll(), Some(false));
        write(&root.join("cpu1/thermal_throttle/core_throttle_count"), "3\n");
        assert_eq!(th.poll(), Some(true), "a counter rose");
        assert_eq!(th.poll(), Some(false), "and it is quiet again");
        write(&root.join("cpu0/thermal_throttle/package_throttle_total_time_ms"), "500\n");
        assert_eq!(th.poll(), Some(true), "time spent throttled counts too");
        // no counters at all (AMD, or a kernel without them)
        assert_eq!(Throttle::default().poll(), None);
        assert!(throttle_files(&root.join("missing")).is_empty());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn usage_math() {
        let a = parse_times("100 0 100 800 0 0 0 0").unwrap();
        let b = parse_times("150 0 150 900 0 0 0 0").unwrap();
        // 100 busy jiffies of 200
        assert_eq!(usage(Some(a), b), Some(50.0));
        assert_eq!(usage(None, b), None);
    }
}
