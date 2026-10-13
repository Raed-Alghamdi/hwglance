//! Threshold alerts: desktop notifications for readings that stay past a threshold.
//!
//! The decision logic ([`Engine`]) takes the time as an argument, so it is tested with a fake
//! clock. Showing the notification ([`Notifier`]) runs `notify-send` on a thread of its own.

use crate::uiprefs::{AlertLevel, AlertScope, AlertSettings};
use hwglance_proto::{Unit, format_value};
use std::collections::HashMap;
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

/// Notification threads alive at once (each waits for its notification to be dismissed).
const MAX_LIVE: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Severity {
    Warning,
    Critical,
}

/// One reading as seen by the engine at one sample.
pub struct Observation<'a> {
    pub id: &'a str,
    pub group: &'a str,
    pub label: &'a str,
    pub unit: Unit,
    pub value: f64,
    pub warn: Option<f64>,
    pub crit: Option<f64>,
    pub below: bool,
    /// Slow readings need fewer consecutive samples (they arrive less often).
    pub slow: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Alert {
    pub id: String,
    pub severity: Severity,
    pub title: String,
    pub body: String,
}

#[derive(Default)]
struct State {
    /// Consecutive observations past the threshold.
    count: u32,
    /// Unix ms of the first of them.
    since_ms: u64,
    /// False after firing, until the value has recovered by the hysteresis.
    armed: bool,
    last_alert_ms: Option<u64>,
}

pub struct Engine {
    states: HashMap<(String, Severity), State>,
}

/// How far back past the threshold a value must go before the alert re-arms.
fn hysteresis(unit: Unit, threshold: f64) -> f64 {
    if unit == Unit::Celsius {
        3.0
    } else {
        threshold.abs() * 0.03
    }
}

fn past(value: f64, threshold: f64, below: bool) -> bool {
    if below {
        value <= threshold
    } else {
        value >= threshold
    }
}

/// "89 °C" rather than "89.0 °C".
fn short(unit: Unit, v: f64) -> String {
    format_value(unit, v).replace(".0 ", " ")
}

impl Engine {
    pub fn new() -> Self {
        Engine {
            states: HashMap::new(),
        }
    }

    /// Feed one observation; returns an alert when one should be shown now.
    pub fn observe(&mut self, now_ms: u64, cfg: &AlertSettings, o: &Observation) -> Option<Alert> {
        if cfg.level == AlertLevel::Off {
            return None;
        }
        if cfg.scope == AlertScope::Temperatures && o.unit != Unit::Celsius {
            return None;
        }
        let mut fired: Option<Alert> = None;
        let levels = [(Severity::Critical, o.crit), (Severity::Warning, o.warn)];
        for (sev, th) in levels {
            if sev == Severity::Warning && cfg.level != AlertLevel::Warning {
                continue;
            }
            let Some(th) = th else { continue };
            let need = if o.slow { 2 } else { 3 };
            let cooldown_ms = (cfg.cooldown_minutes * 60_000.0) as u64;
            let st = self
                .states
                .entry((o.id.to_string(), sev))
                .or_insert_with(|| State {
                    armed: true,
                    ..Default::default()
                });
            if past(o.value, th, o.below) {
                if st.count == 0 {
                    st.since_ms = now_ms;
                }
                st.count += 1;
                // A clock that went back (an NTP step after resume) puts the last alert in the
                // future; waiting for the clock to catch up would silence this reading for as
                // long as the step was, so that counts as cooled.
                let cooled = st
                    .last_alert_ms
                    .is_none_or(|t| t > now_ms || now_ms - t >= cooldown_ms);
                if st.count >= need && st.armed && cooled {
                    st.armed = false;
                    st.last_alert_ms = Some(now_ms);
                    let (word, dir) = (
                        if sev == Severity::Critical {
                            "critical"
                        } else {
                            "warning"
                        },
                        if o.below { "Below" } else { "Above" },
                    );
                    let alert = Alert {
                        id: o.id.to_string(),
                        severity: sev,
                        title: format!("{} {} {}", o.group, o.label, format_value(o.unit, o.value)),
                        body: format!(
                            "{dir} {word} ({}) since {}",
                            short(o.unit, th),
                            hhmm(st.since_ms)
                        ),
                    };
                    // Crossing both levels at once shows only the critical alert.
                    if fired.as_ref().is_none_or(|a| a.severity < sev) {
                        fired = Some(alert);
                    }
                }
            } else {
                st.count = 0;
                let h = hysteresis(o.unit, th);
                let recovered = if o.below {
                    o.value >= th + h
                } else {
                    o.value <= th - h
                };
                if recovered {
                    st.armed = true;
                }
            }
        }
        fired
    }
}

// `Severity` ordering: a critical alert supersedes a warning raised by the same sample.
impl PartialOrd for Severity {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Severity {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (*self == Severity::Critical).cmp(&(*other == Severity::Critical))
    }
}

/// Local time of day, HH:MM.
pub fn hhmm(unix_ms: u64) -> String {
    let secs = (unix_ms / 1000) as libc::time_t;
    // SAFETY: localtime_r only writes into the tm we pass.
    let tm = unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&secs, &mut tm);
        tm
    };
    format!("{:02}:{:02}", tm.tm_hour, tm.tm_min)
}

// ---------------------------------------------------------------------------
// Showing notifications
// ---------------------------------------------------------------------------

/// Called with the reading id when the user clicks "Open HW Glance".
pub type OnOpen = Arc<dyn Fn(&str) + Send + Sync>;

pub struct Notifier {
    live: Arc<AtomicUsize>,
    on_open: OnOpen,
}

/// Run notify-send for an alert and return what it printed (the chosen action).
pub fn show(alert: &Alert, extra: &[&str]) -> std::io::Result<String> {
    let urgency = if alert.severity == Severity::Critical {
        "critical"
    } else {
        "normal"
    };
    let mut child = Command::new("notify-send")
        .args([
            "-a",
            "HW Glance",
            "-u",
            urgency,
            "-i",
            "utilities-system-monitor",
        ])
        .args(["-A", "open=Open HW Glance", "--wait"])
        .args(extra)
        .arg(&alert.title)
        .arg(&alert.body)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let mut out = String::new();
    if let Some(mut so) = child.stdout.take() {
        so.read_to_string(&mut out)?;
    }
    child.wait()?;
    Ok(out.trim().to_string())
}

impl Notifier {
    pub fn new(on_open: OnOpen) -> Self {
        Notifier {
            live: Arc::new(AtomicUsize::new(0)),
            on_open,
        }
    }

    pub fn notify(&self, alert: Alert) {
        if self.live.fetch_add(1, Ordering::AcqRel) >= MAX_LIVE {
            self.live.fetch_sub(1, Ordering::AcqRel);
            return;
        }
        let live = self.live.clone();
        let on_open = self.on_open.clone();
        let spawned = thread::Builder::new().name("notify".into()).spawn(move || {
            match show(&alert, &[]) {
                Ok(action) if action == "open" => on_open(&alert.id),
                Ok(_) => {}
                Err(e) => eprintln!("hwglanced: notify-send failed: {e}"),
            }
            live.fetch_sub(1, Ordering::AcqRel);
        });
        if spawned.is_err() {
            self.live.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

/// May this reading id go into the launch command? It ends up in a shell command line inside
/// omarchy-launch-or-focus, so only the characters sensor ids are made of pass.
fn focus_id_ok(id: &str) -> bool {
    !id.is_empty() && id.len() <= 200 && id.chars().all(|c| c.is_ascii_alphanumeric() || "/_.:-".contains(c))
}

/// Focus the window on a reading: an open window is raised, otherwise one is launched
/// (`hwglance --focus <id>` jumps to the reading either way through the Focus message).
pub fn launch_or_focus(id: &str) {
    if !focus_id_ok(id) {
        return;
    }
    let Some(home) = std::env::var_os("HOME") else {
        return;
    };
    let cmd = format!(
        "{}/.local/bin/hwglance --focus {id}",
        home.to_string_lossy()
    );
    let _ = Command::new("omarchy-launch-or-focus")
        .args(["hwglance", &cmd])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|mut c| thread::spawn(move || c.wait()));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> AlertSettings {
        AlertSettings {
            level: AlertLevel::Critical,
            scope: AlertScope::Temperatures,
            cooldown_minutes: 10.0,
        }
    }

    fn temp(value: f64) -> Observation<'static> {
        Observation {
            id: "cpu/temp/tctl",
            group: "CPU",
            label: "Tctl",
            unit: Unit::Celsius,
            value,
            warn: Some(80.0),
            crit: Some(89.0),
            below: false,
            slow: false,
        }
    }

    const MIN: u64 = 60_000;

    #[test]
    fn a_clock_stepped_back_does_not_silence_a_reading_for_the_length_of_the_step() {
        let mut e = Engine::new();
        let hour = 60 * MIN;
        let c = cfg();
        let mut now = 10 * hour;
        // the first alert fires after three samples
        let mut fired = 0;
        for _ in 0..3 {
            fired += e.observe(now, &c, &temp(95.0)).is_some() as u32;
            now += 1000;
        }
        assert_eq!(fired, 1);
        // it recovers, then the clock is stepped back by two hours and it goes critical again
        for _ in 0..3 {
            e.observe(now, &c, &temp(60.0));
            now += 1000;
        }
        now -= 2 * hour;
        let mut fired = 0;
        for _ in 0..3 {
            fired += e.observe(now, &c, &temp(95.0)).is_some() as u32;
            now += 1000;
        }
        assert_eq!(fired, 1, "not held back until the old clock time comes round again");
    }

    #[test]
    fn only_plain_sensor_ids_reach_the_launch_command() {
        for ok in ["cpu/temp/tctl", "gpu0/temp/hotspot", "disk/S1A2B3C4D5E6F7G/temp", "nvme0/temp:1", "a.b-c_d"] {
            assert!(focus_id_ok(ok), "{ok}");
        }
        for bad in ["", "x; rm -rf ~", "a b", "$(reboot)", "`id`", "a\nb", "a'b", "a\"b", "a|b", "a&b", "../..//x y", &"a".repeat(201)] {
            assert!(!focus_id_ok(bad), "{bad:?}");
        }
    }

    #[test]
    fn needs_three_consecutive_samples() {
        let mut e = Engine::new();
        let c = cfg();
        assert!(e.observe(0, &c, &temp(90.0)).is_none());
        assert!(e.observe(1000, &c, &temp(90.0)).is_none());
        // a dip resets the count
        assert!(e.observe(2000, &c, &temp(80.0)).is_none());
        assert!(e.observe(3000, &c, &temp(90.0)).is_none());
        assert!(e.observe(4000, &c, &temp(90.0)).is_none());
        let a = e
            .observe(5000, &c, &temp(91.0))
            .expect("third sample in a row");
        assert_eq!(a.severity, Severity::Critical);
        assert_eq!(a.id, "cpu/temp/tctl");
        assert_eq!(a.title, "CPU Tctl 91.0 °C");
        assert!(
            a.body.starts_with("Above critical (89 °C) since "),
            "{}",
            a.body
        );
    }

    #[test]
    fn slow_readings_need_two() {
        let mut e = Engine::new();
        let c = cfg();
        let mut o = temp(95.0);
        o.slow = true;
        assert!(e.observe(0, &c, &o).is_none());
        assert!(e.observe(2000, &c, &o).is_some());
    }

    #[test]
    fn exactly_at_threshold_counts_and_only_fires_once() {
        let mut e = Engine::new();
        let c = cfg();
        for t in 0..2 {
            assert!(e.observe(t * 1000, &c, &temp(89.0)).is_none());
        }
        assert!(e.observe(2000, &c, &temp(89.0)).is_some());
        for t in 3..60 {
            assert!(
                e.observe(t * 1000, &c, &temp(95.0)).is_none(),
                "still past: no repeat"
            );
        }
    }

    #[test]
    fn rearms_only_after_hysteresis_and_respects_cooldown() {
        let mut e = Engine::new();
        let c = cfg();
        let hot = |e: &mut Engine, t0: u64| {
            let mut got = None;
            for k in 0..3 {
                got = e.observe(t0 + k * 1000, &c, &temp(92.0));
            }
            got
        };
        assert!(hot(&mut e, 0).is_some());
        // drops to 87: below the threshold but not by 3 degrees, so it does not re-arm
        for k in 0..5 {
            e.observe(10_000 + k * 1000, &c, &temp(87.5));
        }
        assert!(hot(&mut e, 20_000).is_none(), "not re-armed (87.5 > 86)");
        // drops to 85: re-armed, but the 10 minute cooldown has not passed
        for k in 0..5 {
            e.observe(30_000 + k * 1000, &c, &temp(85.0));
        }
        assert!(hot(&mut e, 40_000).is_none(), "cooldown");
        // after the cooldown, still re-armed: the next sustained excursion alerts again
        for k in 0..5 {
            e.observe(11 * MIN + k * 1000, &c, &temp(85.0));
        }
        assert!(hot(&mut e, 11 * MIN + 10_000).is_some());
    }

    #[test]
    fn warning_level_adds_warn_alerts_and_critical_wins() {
        let mut e = Engine::new();
        let mut c = cfg();
        // critical level ignores the warn threshold
        for k in 0..5 {
            assert!(e.observe(k * 1000, &c, &temp(82.0)).is_none());
        }
        c.level = AlertLevel::Warning;
        assert!(e.observe(6000, &c, &temp(82.0)).is_none());
        assert!(e.observe(7000, &c, &temp(82.0)).is_none());
        let a = e.observe(8000, &c, &temp(82.0)).expect("warn alert");
        assert_eq!(a.severity, Severity::Warning);
        assert!(a.body.starts_with("Above warning (80 °C)"));
        // later it passes crit too: its own level alerts separately
        for k in 0..2 {
            assert!(e.observe(10_000 + k * 1000, &c, &temp(90.0)).is_none());
        }
        let a = e.observe(12_000, &c, &temp(90.0)).expect("crit alert");
        assert_eq!(a.severity, Severity::Critical);
    }

    #[test]
    fn below_flips_direction_and_scope_filters() {
        let mut e = Engine::new();
        let mut c = cfg();
        c.scope = AlertScope::All;
        let fan = |v: f64| Observation {
            id: "board/fan/nct6799/fan4",
            group: "Motherboard",
            label: "Rear exhaust",
            unit: Unit::Rpm,
            value: v,
            warn: Some(800.0),
            crit: Some(300.0),
            below: true,
            slow: false,
        };
        assert!(e.observe(0, &c, &fan(200.0)).is_none());
        assert!(e.observe(1000, &c, &fan(200.0)).is_none());
        let a = e.observe(2000, &c, &fan(0.0)).unwrap();
        assert!(a.body.starts_with("Below critical (300 RPM)"), "{}", a.body);
        // re-arm needs 3 % of 300 = 9 RPM above the threshold
        for k in 0..3 {
            e.observe(3000 + k * 1000, &c, &fan(305.0));
        }
        let mut e2 = Engine::new();
        c.scope = AlertScope::Temperatures;
        assert!(
            e2.observe(0, &c, &fan(0.0)).is_none(),
            "RPM is out of scope"
        );
        c.level = AlertLevel::Off;
        assert!(Engine::new().observe(0, &c, &temp(120.0)).is_none());
    }

    #[test]
    fn an_override_replaces_the_defaults_as_a_whole() {
        use hwglance_proto::{Reading, Section, SensorGroup};
        // The file gives Tctl a critical limit of 85 and no warning limit; the defaults (80 and 89)
        // are gone, not merged: a warning at 82 never fires.
        let dir = std::env::temp_dir().join(format!("hwglance-alert-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("thresholds.json");
        std::fs::write(&path, r#"{"readings":{"cpu/temp/tctl":{"warn":null,"crit":85}}}"#).unwrap();
        let mut t = crate::thresholds::Thresholds::new(Some(path), false);
        let mut groups = vec![SensorGroup {
            id: "cpu".into(),
            title: "CPU".into(),
            subtitle: None,
            sections: vec![Section {
                id: "cpu/temp".into(),
                title: "Temperatures".into(),
                collapsed_by_default: false,
                readings: vec![Reading {
                    id: "cpu/temp/tctl".into(),
                    label: "Tctl".into(),
                    unit: Unit::Celsius,
                    value: Some(0.0),
                    stats: None,
                    note: None,
                    desc: None,
                    events: None,
                    warn: Some(80.0),
                    crit: Some(89.0),
                }],
                summary: None,
            }],
        }];
        t.sync(&mut groups, &Default::default());
        let r = &groups[0].sections[0].readings[0];
        let obs = |value: f64| Observation {
            id: &r.id,
            group: "CPU",
            label: &r.label,
            unit: r.unit,
            value,
            warn: r.warn,
            crit: r.crit,
            below: t.below(&r.id),
            slow: false,
        };
        let mut c = cfg();
        c.level = AlertLevel::Warning;
        let mut e = Engine::new();
        for k in 0..5 {
            assert!(e.observe(k * 1000, &c, &obs(82.0)).is_none(), "no warning level, so 82 is fine");
        }
        let mut fired = None;
        for k in 5..9 {
            fired = fired.or(e.observe(k * 1000, &c, &obs(86.0)));
        }
        assert!(fired.expect("critical at 85").body.contains("85"), "the replaced critical limit, not 89");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
