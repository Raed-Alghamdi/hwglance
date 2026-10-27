//! Throttling state of a device (from its `Unit::Flag` readings) and the text of flag rows.

use crate::ui_kit::fmt_clock;
use hwglance_proto::{FlagEvents, Reading, SensorGroup, Unit};

/// How long after its last event throttling still counts as recent (amber). Later it keeps its
/// words, count and time but reads in quiet ink: amber that never clears teaches the eye to
/// ignore amber.
pub const THROTTLE_RECENT_MINUTES: u64 = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Throttle {
    /// Active right now.
    Now,
    /// Happened since the stats started but is off now. `recent`: within
    /// [`THROTTLE_RECENT_MINUTES`] of the last event.
    Past { count: Option<u32>, last_unix_ms: Option<u64>, recent: bool },
    Never,
}

/// When the flag was last on: its end when the collector reports one (an episode that lasted
/// longer than the window is still recent when it has just ended), else its onset.
pub fn last_active_ms(e: &FlagEvents) -> u64 {
    e.ended_unix_ms.unwrap_or(0).max(e.last_unix_ms)
}

/// Is an event at `last_unix_ms` recent at `now_ms`? Unknown times are not.
pub fn is_recent(last_unix_ms: Option<u64>, now_ms: u64) -> bool {
    last_unix_ms.is_some_and(|t| t > 0 && now_ms.saturating_sub(t) <= THROTTLE_RECENT_MINUTES * 60_000)
}

/// The thermal-throttling flag of a card, if the collector reports one.
pub fn find_thermal(g: &SensorGroup) -> Option<&Reading> {
    g.sections
        .iter()
        .flat_map(|s| s.readings.iter())
        .find(|r| r.unit == Unit::Flag && r.id.ends_with("/throttle/thermal"))
}

/// Can this flag mean trouble? Only when the collector gives it a limit (`crit`, or `warn`, set to 1 for
/// thermal throttling). A flag without one, like "Power limit reached", is normal operation.
pub fn can_alarm(r: &Reading) -> bool {
    r.crit.is_some() || r.warn.is_some()
}

/// Is the flag on and past its limit right now?
pub fn alarming_now(r: &Reading) -> bool {
    r.value.is_some_and(|v| r.crit.is_some_and(|c| v >= c) || r.warn.is_some_and(|w| v >= w))
}

/// Has the flag been on at any time since the stats started?
pub fn happened(r: &Reading) -> bool {
    r.events.is_some_and(|e| e.count > 0) || r.stats.is_some_and(|s| s.max >= 0.5)
}

pub fn throttle_state(r: &Reading, now_ms: u64) -> Throttle {
    if r.value.is_some_and(|v| v >= 0.5) {
        Throttle::Now
    } else if happened(r) {
        let recent = is_recent(r.events.map(|e| last_active_ms(&e)), now_ms);
        Throttle::Past { count: r.events.map(|e| e.count), last_unix_ms: r.events.map(|e| e.last_unix_ms), recent }
    } else {
        Throttle::Never
    }
}

pub fn pill_text(t: Throttle) -> String {
    match t {
        Throttle::Never => "No throttling".to_string(),
        Throttle::Now => "Throttling now".to_string(),
        Throttle::Past { count: Some(n), last_unix_ms: Some(ms), .. } => format!("Throttled {n}x, last {}", fmt_clock(ms)),
        Throttle::Past { .. } => "Throttled earlier".to_string(),
    }
}

/// Text in the bar's place on a flag row, and whether the event is recent (warning colour; an
/// older one keeps its words in quiet ink).
pub fn flag_detail(r: &Reading, stats_since_ms: u64, now_ms: u64) -> (String, bool) {
    let (text, recent) = flag_detail_text(r, stats_since_ms, now_ms);
    (text, recent && can_alarm(r))
}

fn flag_detail_text(r: &Reading, stats_since_ms: u64, now_ms: u64) -> (String, bool) {
    if !happened(r) {
        return (format!("never since {}", fmt_clock(stats_since_ms)), false);
    }
    match r.events {
        Some(e) if e.count > 0 => (format!("{}x, last {}", e.count, fmt_clock(e.last_unix_ms)), is_recent(Some(last_active_ms(&e)), now_ms)),
        _ => ("happened earlier".to_string(), false),
    }
}

/// The short form of `flag_detail` for narrow cards.
pub fn flag_detail_short(r: &Reading) -> String {
    match r.events {
        Some(e) if e.count > 0 => format!("{}x", e.count),
        _ if happened(r) => "yes".to_string(),
        _ => "never".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hwglance_proto::{FlagEvents, Section, Stats};

    /// The thermal flag, which the collector marks with `crit` 1.
    fn flag(value: f64, max: f64, events: Option<FlagEvents>) -> Reading {
        Reading { crit: Some(1.0), ..plain_flag(value, max, events) }
    }

    fn plain_flag(value: f64, max: f64, events: Option<FlagEvents>) -> Reading {
        Reading {
            id: "cpu/throttle/thermal".into(),
            label: "Thermal throttling".into(),
            unit: Unit::Flag,
            value: Some(value),
            stats: Some(Stats { min: 0.0, max, avg: 0.1, samples: 100 }),
            note: None,
            desc: None,
            events,
            warn: None,
            crit: None,
        }
    }

    const NOW: u64 = 1_700_000_000_000;
    const MIN: u64 = 60_000;

    #[test]
    fn past_throttling_is_amber_for_a_while_and_then_quiet() {
        let last = NOW - 3 * MIN;
        let r = flag(0.0, 1.0, Some(FlagEvents { count: 3, last_unix_ms: last, ended_unix_ms: None }));
        let recent = |now: u64| matches!(throttle_state(&r, now), Throttle::Past { recent: true, .. });
        assert!(recent(NOW), "3 minutes after the last event");
        assert!(recent(last + THROTTLE_RECENT_MINUTES * MIN), "the window is inclusive");
        assert!(!recent(last + THROTTLE_RECENT_MINUTES * MIN + 1), "just past it");
        assert!(!recent(NOW + 3 * 3_600_000), "hours later");
        // the words, the count and the time stay when it goes quiet
        let later = NOW + 30 * MIN;
        assert_eq!(pill_text(throttle_state(&r, later)), pill_text(throttle_state(&r, NOW)));
        assert!(pill_text(throttle_state(&r, later)).starts_with("Throttled 3x, last "));
        // the row note follows the same clock
        assert!(flag_detail(&r, 0, NOW).1);
        let (text, hot) = flag_detail(&r, 0, later);
        assert!(!hot && text.starts_with("3x, last "), "{text}");
        // a long episode counts from its end, not its onset
        let long = flag(0.0, 1.0, Some(FlagEvents { count: 1, last_unix_ms: NOW - 40 * MIN, ended_unix_ms: Some(NOW - 2 * MIN) }));
        assert!(matches!(throttle_state(&long, NOW), Throttle::Past { recent: true, .. }));
        assert!(flag_detail(&long, 0, NOW).1);
        assert!(!flag_detail(&long, 0, NOW + 20 * MIN).1);
        // a new event makes it amber again
        let again = flag(0.0, 1.0, Some(FlagEvents { count: 4, last_unix_ms: later - MIN, ended_unix_ms: None }));
        assert!(flag_detail(&again, 0, later).1);
        // active throttling is not part of this: it is red whatever the clock says
        assert_eq!(throttle_state(&flag(1.0, 1.0, Some(FlagEvents { count: 1, last_unix_ms: 1, ended_unix_ms: None })), later), Throttle::Now);
    }

    #[test]
    fn the_three_pill_states() {
        let never = flag(0.0, 0.0, Some(FlagEvents { count: 0, last_unix_ms: 0, ended_unix_ms: None }));
        assert_eq!(throttle_state(&never, NOW), Throttle::Never);
        assert_eq!(pill_text(Throttle::Never), "No throttling");

        let now = flag(1.0, 1.0, Some(FlagEvents { count: 3, last_unix_ms: 5, ended_unix_ms: None }));
        assert_eq!(throttle_state(&now, NOW), Throttle::Now);
        assert_eq!(pill_text(Throttle::Now), "Throttling now");

        let past = flag(0.0, 1.0, Some(FlagEvents { count: 2, last_unix_ms: 1_700_000_000_000, ended_unix_ms: None }));
        match throttle_state(&past, NOW) {
            Throttle::Past { count: Some(2), last_unix_ms: Some(_), .. } => {}
            other => panic!("{other:?}"),
        }
        let text = pill_text(throttle_state(&past, NOW));
        assert!(text.starts_with("Throttled 2x, last "), "{text}");
        assert_eq!(text.len(), "Throttled 2x, last 21:14".len());
    }

    #[test]
    fn past_without_event_counts_still_reads_sensibly() {
        let r = flag(0.0, 1.0, None);
        assert_eq!(pill_text(throttle_state(&r, NOW)), "Throttled earlier");
        assert!(matches!(throttle_state(&r, NOW), Throttle::Past { recent: false, .. }), "no time, no amber");
        assert!(happened(&r));
    }

    #[test]
    fn flag_row_detail() {
        let never = flag(0.0, 0.0, None);
        let (text, hot) = flag_detail(&never, 1_700_000_000_000, NOW);
        assert!(text.starts_with("never since "));
        assert!(!hot);
        let past = flag(0.0, 1.0, Some(FlagEvents { count: 3, last_unix_ms: 1_700_000_100_000, ended_unix_ms: None }));
        let (text, hot) = flag_detail(&past, 0, 1_700_000_100_000 + 60_000);
        assert!(text.starts_with("3x, last "));
        assert!(hot);
    }

    #[test]
    fn only_a_flag_with_a_limit_can_alarm() {
        let thermal = flag(1.0, 1.0, None);
        assert!(can_alarm(&thermal) && alarming_now(&thermal));
        assert!(!alarming_now(&flag(0.0, 1.0, None)));
        // "Power limit reached" has no limit: on is normal operation
        let power = plain_flag(1.0, 1.0, None);
        assert!(!can_alarm(&power) && !alarming_now(&power));
        let ev = Some(FlagEvents { count: 3, last_unix_ms: NOW - 3 * MIN, ended_unix_ms: None });
        assert!(flag_detail(&flag(0.0, 1.0, ev), 0, NOW).1, "a recent thermal event is amber");
        let (text, recent) = flag_detail(&plain_flag(0.0, 1.0, ev), 0, NOW);
        assert!(text.starts_with("3x, last ") && !recent, "a recent power event keeps its words in quiet ink");
    }

    #[test]
    fn short_forms_for_narrow_rows() {
        assert_eq!(flag_detail_short(&flag(0.0, 0.0, None)), "never");
        assert_eq!(flag_detail_short(&flag(0.0, 1.0, Some(FlagEvents { count: 3, last_unix_ms: 1, ended_unix_ms: None }))), "3x");
        assert_eq!(flag_detail_short(&flag(0.0, 1.0, None)), "yes");
    }

    #[test]
    fn thermal_flag_is_found_in_a_card() {
        let g = SensorGroup {
            id: "cpu".into(),
            title: "CPU".into(),
            subtitle: None,
            sections: vec![Section {
                id: "cpu/throttle".into(),
                title: "Limits".into(),
                collapsed_by_default: false,
                readings: vec![flag(0.0, 0.0, None)],
                summary: None,
            }],
        };
        assert!(find_thermal(&g).is_some());
        let mut other = g.clone();
        other.sections[0].readings[0].id = "gpu0/throttle/power".into();
        assert!(find_thermal(&other).is_none());
    }
}
