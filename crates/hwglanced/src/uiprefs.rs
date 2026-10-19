//! `~/.config/hwglance/ui.json`, written by the window and only read here: how much history to
//! keep, the alert settings and the CSV log choices. Polled by modification time; a missing or
//! half-written file leaves the previous settings in place. The threshold overrides it used to
//! hold (`sensors.thresholds`) are read only to be folded into `thresholds.json` once (see
//! `thresholds.rs`).

use crate::thresholds::LegacyLimits;
use hwglance_proto::{LOG_INTERVAL_S, LOG_KEEP_DAYS};
use serde::Deserialize;
use serde_json::Value;
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

/// What goes into the CSV log (the `logging` object of ui.json). It is active when `all` is on or
/// `values` is not empty.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogPrefs {
    /// Every reading the collector has, hidden ones included.
    pub all: bool,
    /// Reading ids in the order they were ticked.
    pub values: Vec<String>,
    /// Empty means the default folder; see [`hwglance_proto::log_folder`].
    pub folder: String,
    /// Seconds between rows, 1 to 3600.
    pub interval_s: u32,
    /// Finished files are gzipped (to `.csv.gz`) by a background thread.
    pub compress: bool,
    /// Log files older than this many days are deleted; 0 keeps everything.
    pub keep_days: u32,
}

impl Default for LogPrefs {
    fn default() -> Self {
        LogPrefs { all: false, values: Vec::new(), folder: String::new(), interval_s: 1, compress: true, keep_days: 0 }
    }
}

impl LogPrefs {
    pub fn active(&self) -> bool {
        self.all || !self.values.is_empty()
    }

    /// Read the object field by field: a field of the wrong type (or a `logging` that is not an
    /// object) takes its default and nothing else is lost, so a damaged log choice never costs
    /// the alert or history settings that share the file.
    fn from_value(v: Option<&Value>) -> LogPrefs {
        let Some(o) = v.and_then(Value::as_object) else { return LogPrefs::default() };
        let d = LogPrefs::default();
        let number = |key: &str| o.get(key).and_then(Value::as_f64).filter(|x| x.is_finite());
        let clamped = |x: f64, range: &std::ops::RangeInclusive<u32>| x.round().clamp(f64::from(*range.start()), f64::from(*range.end())) as u32;
        LogPrefs {
            all: o.get("all").and_then(Value::as_bool).unwrap_or(d.all),
            values: o
                .get("values")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
                .unwrap_or(d.values),
            folder: o.get("folder").and_then(Value::as_str).map(str::to_string).unwrap_or(d.folder),
            interval_s: number("interval_seconds").map_or(d.interval_s, |x| clamped(x, &LOG_INTERVAL_S)),
            compress: o.get("compress").and_then(Value::as_bool).unwrap_or(d.compress),
            keep_days: number("keep_days").map_or(d.keep_days, |x| clamped(x, &LOG_KEEP_DAYS)),
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
    logging: Option<Value>,
}

#[derive(Clone, Debug)]
pub struct Prefs {
    pub history_minutes: u32,
    /// Overrides of the old format, for the one-time migration only.
    pub legacy_thresholds: HashMap<String, LegacyLimits>,
    pub alerts: AlertSettings,
    pub logging: LogPrefs,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs {
            history_minutes: 30,
            legacy_thresholds: HashMap::new(),
            alerts: AlertSettings::default(),
            logging: LogPrefs::default(),
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
            logging: LogPrefs::from_value(raw.logging.as_ref()),
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
        assert_eq!(p.logging, LogPrefs::default());
        assert!(!p.logging.active());
    }

    #[test]
    fn reads_the_logging_choices_and_clamps_the_interval() {
        let p = Prefs::parse(
            r#"{"logging":{"all":false,"values":["cpu/temp/tctl","gpu0/load/core"],"folder":"~/hw","interval_seconds":5,"future":1}}"#,
        )
        .unwrap();
        assert_eq!(p.logging.values, ["cpu/temp/tctl", "gpu0/load/core"], "tick order is kept");
        assert_eq!((p.logging.folder.as_str(), p.logging.interval_s, p.logging.all), ("~/hw", 5, false));
        assert!(p.logging.active());
        let all = |t: &str| Prefs::parse(&format!(r#"{{"logging":{t}}}"#)).unwrap().logging;
        assert!(all(r#"{"all":true}"#).active(), "all alone is enough");
        assert_eq!(all(r#"{"interval_seconds":0}"#).interval_s, 1);
        assert_eq!(all(r#"{"interval_seconds":999999}"#).interval_s, 3600);
        assert_eq!(all(r#"{"interval_seconds":-4}"#).interval_s, 1);
        assert_eq!(all(r#"{"interval_seconds":2.4}"#).interval_s, 2);
        assert_eq!(all("{}"), LogPrefs::default());
    }

    #[test]
    fn compression_is_on_and_the_age_limit_off_unless_the_file_says_otherwise() {
        let d = Prefs::parse(r#"{"logging":{"all":true}}"#).unwrap().logging;
        assert!(d.compress && d.keep_days == 0);
        let p = |t: &str| Prefs::parse(&format!(r#"{{"logging":{t}}}"#)).unwrap().logging;
        assert!(!p(r#"{"compress":false}"#).compress);
        assert!(p(r#"{"compress":"no"}"#).compress, "a wrong type takes the default");
        assert_eq!(p(r#"{"keep_days":30}"#).keep_days, 30);
        assert_eq!(p(r#"{"keep_days":99999}"#).keep_days, 3650);
        assert_eq!(p(r#"{"keep_days":-5}"#).keep_days, 0);
        assert_eq!(p(r#"{"keep_days":"7"}"#).keep_days, 0);
        assert_eq!(p(r#"{"keep_days":6.6}"#).keep_days, 7);
    }

    #[test]
    fn a_damaged_logging_object_does_not_cost_the_other_settings() {
        let rest = r#""alerts":{"level":"warning","cooldown_minutes":7},"sensors":{"history_minutes":60}"#;
        for logging in [r#""logging":"on""#, r#""logging":[1,2]"#, r#""logging":null"#, r#""logging":{"all":"yes","values":"cpu/temp/tctl","folder":7,"interval_seconds":"5"}"#] {
            let p = Prefs::parse(&format!("{{{logging},{rest}}}")).unwrap();
            assert_eq!(p.logging, LogPrefs::default(), "{logging}");
            assert_eq!((p.alerts.level, p.alerts.cooldown_minutes, p.history_minutes), (AlertLevel::Warning, 7.0, 60), "{logging}");
        }
        // a wrong field keeps its default and the right ones stand; non-string ids are dropped
        let p = Prefs::parse(r#"{"logging":{"all":true,"values":["a/b/c",3,null],"folder":false,"interval_seconds":"x"}}"#).unwrap();
        assert!(p.logging.all && p.logging.folder.is_empty() && p.logging.interval_s == 1);
        assert_eq!(p.logging.values, ["a/b/c"]);
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
