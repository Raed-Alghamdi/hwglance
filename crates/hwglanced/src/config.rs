//! Optional `~/.config/hwglance/config.toml`.

use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

#[derive(Deserialize, Default)]
struct RawHide {
    #[serde(default)]
    ids: Vec<String>,
}

#[derive(Deserialize, Default)]
struct Raw {
    interval_ms: Option<u64>,
    slow_interval_ms: Option<u64>,
    cpu_thermal_limit_c: Option<f64>,
    text_readouts: Option<bool>,
    #[serde(default)]
    labels: HashMap<String, String>,
    #[serde(default)]
    hide: RawHide,
}

pub struct Config {
    pub interval_ms: u64,
    pub slow_interval_ms: u64,
    /// Temperature at which the CPU lowers its clocks; `None` takes the limit the hardware
    /// reports, else AMD's published figure for the model, else 95.
    pub cpu_thermal_limit_c: Option<f64>,
    /// Write the plain-text readouts (`textexport`) every sample. Off unless asked for.
    pub text_readouts: bool,
    /// `chip/sensor` -> display label.
    pub labels: HashMap<String, String>,
    pub hide: HashSet<String>,
}

impl Config {
    pub fn path() -> Option<PathBuf> {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
        Some(base.join("hwglance").join("config.toml"))
    }

    pub fn load() -> Config {
        let raw = Self::path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .map(|text| {
                toml::from_str::<Raw>(&text).unwrap_or_else(|e| {
                    eprintln!("hwglanced: ignoring config.toml: {e}");
                    Raw::default()
                })
            })
            .unwrap_or_default();
        Self::from_raw(raw)
    }

    fn from_raw(raw: Raw) -> Config {
        Config {
            interval_ms: raw.interval_ms.unwrap_or(1000).clamp(100, 60_000),
            slow_interval_ms: raw.slow_interval_ms.unwrap_or(2000).clamp(250, 600_000),
            cpu_thermal_limit_c: raw
                .cpu_thermal_limit_c
                .filter(|c| (40.0..=120.0).contains(c)),
            text_readouts: raw.text_readouts.unwrap_or(false),
            labels: raw.labels,
            hide: raw.hide.ids.into_iter().collect(),
        }
    }

    #[cfg(test)]
    pub fn defaults() -> Config {
        Self::from_raw(Raw::default())
    }

    #[cfg(test)]
    pub fn for_test(labels: &[(&str, &str)], cpu_thermal_limit_c: Option<f64>) -> Config {
        let mut c = Self::defaults();
        c.labels = labels.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        c.cpu_thermal_limit_c = cpu_thermal_limit_c;
        c
    }

    pub fn label(&self, chip: &str, key: &str) -> Option<&str> {
        self.labels
            .get(&format!("{chip}/{key}"))
            .map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_override() {
        let raw: Raw = toml::from_str(
            "interval_ms = 500\n[labels]\n\"nct6799/fan4\" = \"Back\"\n[hide]\nids = [\"cpu/temp/tccd1\"]\n",
        )
        .unwrap();
        let c = Config::from_raw(raw);
        assert_eq!(c.interval_ms, 500);
        assert_eq!(c.slow_interval_ms, 2000);
        assert_eq!(c.label("nct6799", "fan4"), Some("Back"));
        // no built-in names: a fan without a label of its own is "Fan N"
        assert_eq!(c.label("nct6799", "fan1"), None);
        assert_eq!(c.cpu_thermal_limit_c, None);
        assert!(c.hide.contains("cpu/temp/tccd1"));
    }

    #[test]
    fn limit_override_is_range_checked() {
        let c = |s: &str| Config::from_raw(toml::from_str(s).unwrap()).cpu_thermal_limit_c;
        assert_eq!(c("cpu_thermal_limit_c = 89"), Some(89.0));
        assert_eq!(c("cpu_thermal_limit_c = 12"), None);
    }

    #[test]
    fn text_readouts_default_off() {
        let c = |s: &str| Config::from_raw(toml::from_str(s).unwrap()).text_readouts;
        assert!(!c(""));
        assert!(c("text_readouts = true"));
        assert!(!c("text_readouts = false"));
    }

    #[test]
    fn example_config_parses() {
        let raw: Raw =
            toml::from_str(include_str!("../../../packaging/config.example.toml")).unwrap();
        let c = Config::from_raw(raw);
        assert_eq!(c.interval_ms, 1000);
        assert!(!c.text_readouts);
        assert!(c.hide.is_empty());
    }
}
