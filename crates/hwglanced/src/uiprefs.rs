//! `~/.config/hwglance/ui.json`, written by the window and only read here: how much history to
//! keep and the alert settings. Polled by modification time; a missing or half-written file leaves
//! the previous settings in place. The threshold overrides it used to hold (`sensors.thresholds`)
//! are read only to be folded into `thresholds.json` once (see `thresholds.rs`).

use crate::thresholds::LegacyLimits;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

const POLL_EVERY: Duration = Duration::from_secs(2);

#[derive(Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AlertLevel {
    Off,
    Critical,
    Warning,
}

#[derive(Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AlertScope {
    Temperatures,
    All,
}

#[derive(Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(default)]
pub struct AlertSettings {
    pub level: AlertLevel,
    pub scope: AlertScope,
    pub cooldown_minutes: f64,
}

impl Default for AlertSettings {
    fn default() -> Self {
        AlertSettings {
            level: AlertLevel::Critical,
            scope: AlertScope::Temperatures,
            cooldown_minutes: 10.0,
        }
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawSensors {
    history_minutes: Option<u32>,
    thresholds: HashMap<String, LegacyLimits>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Raw {
    sensors: RawSensors,
    alerts: AlertSettings,
}

#[derive(Clone, Debug)]
pub struct Prefs {
    pub history_minutes: u32,
    /// Overrides of the old format, for the one-time migration only.
    pub legacy_thresholds: HashMap<String, LegacyLimits>,
    pub alerts: AlertSettings,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs {
            history_minutes: 30,
            legacy_thresholds: HashMap::new(),
            alerts: AlertSettings::default(),
        }
    }
}

impl Prefs {
    /// Parse the file text. A field of the wrong type fails the whole parse (keep the old settings).
    pub fn parse(text: &str) -> Result<Prefs, serde_json::Error> {
        let raw: Raw = serde_json::from_str(text)?;
        Ok(Prefs {
            history_minutes: raw.sensors.history_minutes.unwrap_or(30).clamp(1, 240),
            legacy_thresholds: raw.sensors.thresholds,
            alerts: AlertSettings {
                cooldown_minutes: raw.alerts.cooldown_minutes.clamp(0.0, 24.0 * 60.0),
                ..raw.alerts
            },
        })
    }
}

pub struct PrefsWatcher {
    path: Option<PathBuf>,
    mtime: Option<SystemTime>,
    last_poll: Option<Instant>,
    pub prefs: Prefs,
}

impl PrefsWatcher {
    pub fn new() -> Self {
        PrefsWatcher {
            path: hwglance_proto::config_dir().map(|d| d.join("ui.json")),
            mtime: None,
            last_poll: None,
            prefs: Prefs::default(),
        }
    }

    /// Re-read the file when its modification time changed (checked at most every 2 s).
    pub fn poll(&mut self) {
        if self.last_poll.is_some_and(|t| t.elapsed() < POLL_EVERY) {
            return;
        }
        self.last_poll = Some(Instant::now());
        let Some(path) = &self.path else { return };
        let Ok(mtime) = std::fs::metadata(path).and_then(|m| m.modified()) else {
            return; // missing: keep what we have
        };
        if self.mtime == Some(mtime) {
            return;
        }
        // Remember the time even when parsing fails, so a broken file is not re-read every 2 s;
        // the window rewrites it (new mtime) once it is whole.
        self.mtime = Some(mtime);
        if let Some(p) = std::fs::read_to_string(path)
            .ok()
            .and_then(|t| Prefs::parse(&t).ok())
        {
            self.prefs = p;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_when_sections_are_missing() {
        let p = Prefs::parse("{}").unwrap();
        assert_eq!(p.history_minutes, 30);
        assert_eq!(p.alerts, AlertSettings::default());
        assert!(p.legacy_thresholds.is_empty());
    }

    #[test]
    fn reads_and_clamps() {
        let p = Prefs::parse(
            r#"{"version":1,"appearance":{"font_size":14},
                "sensors":{"history_minutes":9999,"hidden":[],
                  "thresholds":{"cpu/temp/tctl":{"crit":85.0},"fan/x":{"warn":800,"crit":300,"below":true}}},
                "alerts":{"level":"warning","scope":"all","cooldown_minutes":5}}"#,
        )
        .unwrap();
        assert_eq!(p.history_minutes, 240);
        assert_eq!(p.legacy_thresholds["cpu/temp/tctl"].crit, Some(85.0));
        assert_eq!(p.legacy_thresholds["cpu/temp/tctl"].warn, None);
        assert!(p.legacy_thresholds["fan/x"].below);
        assert_eq!(p.alerts.level, AlertLevel::Warning);
        assert_eq!(p.alerts.scope, AlertScope::All);
        assert_eq!(p.alerts.cooldown_minutes, 5.0);
        assert_eq!(
            Prefs::parse(r#"{"sensors":{"history_minutes":0}}"#)
                .unwrap()
                .history_minutes,
            1
        );
    }

    #[test]
    fn partial_file_is_an_error() {
        assert!(Prefs::parse(r#"{"sensors":{"history_mi"#).is_err());
    }
}
