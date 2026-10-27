//! hwmon discovery and reading. Read only: nothing here ever writes to sysfs.

use crate::util::{read_num, read_trim};
use std::fs;
use std::path::{Path, PathBuf};

/// Chips this daemon shows (the GPUs are read by gpu.rs, the network cards by name below).
pub fn is_known(name: &str) -> bool {
    matches!(
        name,
        "k10temp" | "coretemp" | "asusec" | "spd5118" | "nvme" | "thinkpad" | "dell_smm"
    ) || is_superio(name)
        || is_wifi(name)
}

/// Wi-Fi cards that report a temperature (MediaTek, Intel, Atheros drivers).
pub fn is_wifi(name: &str) -> bool {
    name.starts_with("mt76") || name.starts_with("mt79") || name.starts_with("iwlwifi") || name.starts_with("ath1") || name.starts_with("ath9")
}

/// Chips whose fans belong on the motherboard card: Super I/O chips, the board's embedded
/// controller and the laptop fan drivers.
pub fn has_board_fans(name: &str) -> bool {
    is_superio(name) || matches!(name, "asusec" | "thinkpad" | "dell_smm")
}

/// Chips that talk to a device on every read (a drive, the SPD hub, a radio's firmware, the
/// board's embedded controller through ACPI):
/// they are read on the slow interval only.
fn is_slow(name: &str) -> bool {
    matches!(name, "spd5118" | "nvme" | "asusec") || is_wifi(name)
}

/// Super I/O chips (Nuvoton nct6*, ITE it8*, Fintek f71*, Winbond w83*). One cache refresh of
/// the nct6799 driver costs about 95 ms of CPU (slow port I/O) and the driver refreshes at most
/// every 1.5 s, so these are read on the slow thread and only every few polls there.
/// CoolerControl keeps the driver cache warm.
pub fn is_superio(name: &str) -> bool {
    ["nct6", "it8", "f71", "w83"].iter().any(|p| name.starts_with(p))
}

pub struct Temp {
    pub idx: u32,
    pub input: PathBuf,
    pub label: String,
    pub max: Option<f64>,
    pub crit: Option<f64>,
}

pub struct Fan {
    pub idx: u32,
    pub input: PathBuf,
    pub label: Option<String>,
    /// `pwm{idx}`, the duty the board sends this header, when the driver has one to read.
    pub pwm: Option<PathBuf>,
}

pub struct Chip {
    pub name: String,
    /// Name of the parent device: `10-0050` for an SPD hub, `nvme0` for a drive.
    pub dev: String,
    pub temps: Vec<Temp>,
    pub fans: Vec<Fan>,
    pub slow: bool,
}

#[derive(Clone, Debug)]
pub struct TempVal {
    pub label: String,
    pub value: Option<f64>,
    pub max: Option<f64>,
    pub crit: Option<f64>,
}

#[derive(Clone, Debug)]
pub struct FanVal {
    pub idx: u32,
    pub label: Option<String>,
    pub rpm: Option<f64>,
    /// The PWM duty in percent, for a header with a `pwm` file. Some headers never report a
    /// speed (rpm stays 0), so this is the only sign of life they give.
    pub duty: Option<f64>,
}

#[derive(Clone, Debug, Default)]
pub struct ChipVal {
    pub name: String,
    pub dev: String,
    pub temps: Vec<TempVal>,
    pub fans: Vec<FanVal>,
}

/// Number between `prefix` and `suffix` in a file name (`temp12_input` -> 12).
fn index_of(file: &str, prefix: &str, suffix: &str) -> Option<u32> {
    file.strip_prefix(prefix)?
        .strip_suffix(suffix)?
        .parse()
        .ok()
}

fn milli(p: &Path) -> Option<f64> {
    read_num(p).map(|v| v / 1000.0)
}

/// Resolve a hwmon device to the NVMe controller name (`nvme0`), whichever of the two
/// sysfs layouts the kernel uses (the device link may point at the controller or at the PCI device).
fn nvme_controller(dev_path: &Path) -> Option<String> {
    let fname = dev_path.file_name()?.to_string_lossy().into_owned();
    if fname.starts_with("nvme") {
        return Some(fname);
    }
    for e in fs::read_dir("/sys/class/nvme").ok()?.flatten() {
        if fs::canonicalize(e.path().join("device")).ok().as_deref() == Some(dev_path) {
            return Some(e.file_name().to_string_lossy().into_owned());
        }
    }
    None
}

pub fn discover() -> Vec<Chip> {
    let mut dirs: Vec<(u32, PathBuf)> = fs::read_dir("/sys/class/hwmon")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let n = index_of(&e.file_name().to_string_lossy(), "hwmon", "")?;
            Some((n, e.path()))
        })
        .collect();
    dirs.sort();
    let mut chips = Vec::new();
    for (_, dir) in dirs {
        let Some(name) = read_trim(dir.join("name")) else {
            continue;
        };
        if !is_known(&name) {
            continue;
        }
        let dev_path = fs::canonicalize(dir.join("device")).unwrap_or_else(|_| dir.clone());
        let mut dev = dev_path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        if name == "nvme" {
            dev = nvme_controller(&dev_path).unwrap_or(dev);
        }
        let mut temps = Vec::new();
        let mut fans = Vec::new();
        for e in fs::read_dir(&dir).into_iter().flatten().flatten() {
            let f = e.file_name().to_string_lossy().into_owned();
            if let Some(idx) = index_of(&f, "temp", "_input") {
                let label = read_trim(dir.join(format!("temp{idx}_label")))
                    .unwrap_or_else(|| format!("temp{idx}"));
                temps.push(Temp {
                    idx,
                    input: e.path(),
                    label,
                    max: milli(&dir.join(format!("temp{idx}_max"))),
                    crit: milli(&dir.join(format!("temp{idx}_crit"))),
                });
            } else if let Some(idx) = index_of(&f, "fan", "_input") {
                fans.push(Fan {
                    idx,
                    input: e.path(),
                    label: read_trim(dir.join(format!("fan{idx}_label"))),
                    pwm: pwm_file(&dir, idx),
                });
            }
        }
        temps.sort_by_key(|t| t.idx);
        fans.sort_by_key(|f| f.idx);
        let slow = is_slow(&name) || is_superio(&name);
        chips.push(Chip {
            name,
            dev,
            temps,
            fans,
            slow,
        });
    }
    chips
}

/// The `pwm{idx}` file of a fan, if it exists and can be read (it is only ever read).
fn pwm_file(dir: &Path, idx: u32) -> Option<PathBuf> {
    let p = dir.join(format!("pwm{idx}"));
    read_num(&p).map(|_| p)
}

/// A hwmon `pwm` value (0 to 255) as a percentage.
fn duty_percent(raw: f64) -> f64 {
    (raw / 255.0 * 100.0).clamp(0.0, 100.0)
}

pub fn read_chip(c: &Chip) -> ChipVal {
    ChipVal {
        name: c.name.clone(),
        dev: c.dev.clone(),
        temps: c
            .temps
            .iter()
            .map(|t| TempVal {
                label: t.label.clone(),
                value: milli(&t.input),
                max: t.max,
                crit: t.crit,
            })
            .collect(),
        fans: c
            .fans
            .iter()
            .map(|f| FanVal {
                idx: f.idx,
                label: f.label.clone(),
                rpm: read_num(&f.input),
                duty: f.pwm.as_deref().and_then(read_num).map(duty_percent),
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indices() {
        assert_eq!(index_of("temp12_input", "temp", "_input"), Some(12));
        assert_eq!(index_of("temp_input", "temp", "_input"), None);
        assert_eq!(index_of("hwmon7", "hwmon", ""), Some(7));
        assert_eq!(index_of("name", "hwmon", ""), None);
    }

    #[test]
    fn a_fan_duty_is_read_from_its_pwm_file_as_a_percentage() {
        let dir = std::env::temp_dir().join(format!("hwglance-pwm-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("pwm3"), "178\n").unwrap();
        fs::write(dir.join("pwm4"), "garbage\n").unwrap();
        // only a readable pwm file counts: fan 2 has none and fan 4's is not a number
        assert_eq!(pwm_file(&dir, 3), Some(dir.join("pwm3")));
        assert_eq!(pwm_file(&dir, 2), None);
        assert_eq!(pwm_file(&dir, 4), None);
        let fan = Fan { idx: 3, input: dir.join("fan3_input"), label: None, pwm: pwm_file(&dir, 3) };
        let chip = Chip { name: "nct6799".into(), dev: "nct6775.656".into(), temps: Vec::new(), fans: vec![fan], slow: true };
        fs::write(dir.join("fan3_input"), "0\n").unwrap();
        let v = read_chip(&chip);
        assert_eq!(v.fans[0].rpm, Some(0.0));
        let duty = v.fans[0].duty.unwrap();
        assert!((duty - 69.8).abs() < 0.1, "{duty}");
        // a fan without a pwm file has no duty
        let bare = Chip { fans: vec![Fan { idx: 3, input: dir.join("fan3_input"), label: None, pwm: None }], ..chip };
        assert_eq!(read_chip(&bare).fans[0].duty, None);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn duty_percent_spans_zero_to_one_hundred() {
        assert_eq!(duty_percent(0.0), 0.0);
        assert_eq!(duty_percent(255.0), 100.0);
        assert!((duty_percent(153.0) - 60.0).abs() < 1e-9);
        assert_eq!(duty_percent(300.0), 100.0);
    }

    #[test]
    fn chip_families() {
        for n in ["nct6799", "nct6798", "nct6775", "it8688", "it87", "f71882fg"] {
            assert!(is_known(n) && is_superio(n) && has_board_fans(n), "{n}");
        }
        for n in ["mt7921_phy0", "mt7925_phy0", "iwlwifi_1", "ath11k_hwmon"] {
            assert!(is_known(n) && is_wifi(n) && is_slow(n), "{n}");
        }
        assert!(is_known("coretemp") && !is_slow("coretemp") && !has_board_fans("coretemp"));
        assert!(is_known("thinkpad") && has_board_fans("thinkpad"));
        // the GPUs are not hwmon chips here
        assert!(!is_known("amdgpu") && !is_known("i915") && !is_known("nouveau"));
    }
}
