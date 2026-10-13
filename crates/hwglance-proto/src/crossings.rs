//! Threshold crossings: for every reading, each stretch during which it was past its warning or
//! critical threshold, with the time it began, its worst value and the time it ended. The window
//! shows them after the fact ("2 crossings since 11:30") and keeps nothing itself, so a window
//! that connects later, or reconnects, sees the same list.
//!
//! It lives in the protocol crate so the demo data can use the same rules as the collector.
//!
//! This is independent of the notifications: it records at every alert level, and also when no
//! notifier runs. The decision takes the time as an argument, so it is tested with a fake clock.

use crate::{Crossing, CrossingLevel};

/// A value that is past its threshold again within this long of leaving continues the same
/// crossing, so a reading that flickers around its threshold is one entry, not twenty.
pub const MERGE_GAP_MS: u64 = 30_000;
/// Closed crossings older than this are forgotten.
pub const KEEP_CLOSED_MS: u64 = 6 * 3_600_000;
/// At most this many are kept (the oldest closed ones go first).
pub const MAX_KEPT: usize = 100;

#[derive(Default)]
pub struct Tracker {
    /// Oldest first.
    list: Vec<Crossing>,
}

fn past(value: f64, threshold: f64, below: bool) -> bool {
    if below { value <= threshold } else { value >= threshold }
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

impl Tracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one fresh value of a reading with its effective thresholds.
    pub fn observe(&mut self, now_ms: u64, id: &str, value: f64, warn: Option<f64>, crit: Option<f64>, below: bool) {
        let level = if crit.is_some_and(|t| past(value, t, below)) {
            Some(CrossingLevel::Crit)
        } else if warn.is_some_and(|t| past(value, t, below)) {
            Some(CrossingLevel::Warn)
        } else {
            None
        };
        let last = self.list.iter().rposition(|c| c.id == id);
        let Some(level) = level else {
            if let Some(i) = last {
                self.list[i].end_ms.get_or_insert(now_ms);
            }
            return;
        };
        let continues = last.filter(|&i| self.list[i].end_ms.is_none_or(|end| now_ms.saturating_sub(end) <= MERGE_GAP_MS));
        match continues {
            Some(i) => {
                let c = &mut self.list[i];
                c.end_ms = None;
                c.level = c.level.max(level);
                let worse = if below { value < c.peak } else { value > c.peak };
                if worse {
                    c.peak = round2(value);
                    c.peak_ms = now_ms;
                }
            }
            None => self.list.push(Crossing {
                id: id.to_string(),
                level,
                start_ms: now_ms,
                peak: round2(value),
                peak_ms: now_ms,
                end_ms: None,
                below,
            }),
        }
    }

    /// Forget old closed crossings and cap the list.
    pub fn prune(&mut self, now_ms: u64) {
        self.list.retain(|c| c.end_ms.is_none_or(|end| now_ms.saturating_sub(end) <= KEEP_CLOSED_MS));
        while self.list.len() > MAX_KEPT {
            match self.list.iter().position(|c| c.end_ms.is_some()) {
                Some(i) => {
                    self.list.remove(i);
                }
                None => {
                    self.list.remove(0);
                }
            }
        }
    }

    /// Drop the crossings that are over (Reset); the ones still going stay.
    pub fn clear_closed(&mut self) {
        self.list.retain(|c| c.end_ms.is_none());
    }

    /// Put back crossings that `clear_closed` dropped (an undone reset). What was recorded since
    /// is kept: a crossing that was still going is the same one in both lists, and the newer
    /// record of it wins. Oldest first, as before.
    pub fn absorb_earlier(&mut self, earlier: Vec<Crossing>) {
        let mut merged = earlier;
        for c in self.list.drain(..) {
            match merged.iter_mut().find(|e| e.id == c.id && e.start_ms == c.start_ms) {
                Some(e) => *e = c,
                None => merged.push(c),
            }
        }
        merged.sort_by_key(|c| c.start_ms);
        self.list = merged;
    }

    pub fn list(&self) -> &[Crossing] {
        &self.list
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: u64 = 1000;

    fn feed(t: &mut Tracker, now: u64, v: f64) {
        t.observe(now, "cpu/temp/tctl", v, Some(80.0), Some(89.0), false);
    }

    #[test]
    fn a_crossing_has_a_start_a_peak_and_an_end() {
        let mut t = Tracker::new();
        feed(&mut t, 0, 70.0);
        assert!(t.list().is_empty(), "below the thresholds: nothing recorded");
        feed(&mut t, 10 * S, 82.0);
        feed(&mut t, 11 * S, 85.4321);
        feed(&mut t, 12 * S, 83.0);
        let c = &t.list()[0];
        assert_eq!((c.level, c.start_ms, c.peak, c.peak_ms, c.end_ms), (CrossingLevel::Warn, 10 * S, 85.43, 11 * S, None));
        feed(&mut t, 13 * S, 70.0);
        let c = &t.list()[0];
        assert_eq!(c.end_ms, Some(13 * S), "back inside");
        // staying inside does not move the end
        feed(&mut t, 14 * S, 70.0);
        assert_eq!(t.list()[0].end_ms, Some(13 * S));
        assert_eq!(t.list().len(), 1);
    }

    #[test]
    fn the_level_is_the_worst_reached() {
        let mut t = Tracker::new();
        feed(&mut t, 0, 82.0);
        assert_eq!(t.list()[0].level, CrossingLevel::Warn);
        feed(&mut t, S, 91.0);
        feed(&mut t, 2 * S, 84.0);
        let c = &t.list()[0];
        assert_eq!((c.level, c.peak, c.peak_ms), (CrossingLevel::Crit, 91.0, S), "one crossing, upgraded");
        assert_eq!(t.list().len(), 1);
    }

    #[test]
    fn a_quick_return_continues_the_same_crossing_and_a_late_one_starts_another() {
        let mut t = Tracker::new();
        feed(&mut t, 0, 85.0);
        feed(&mut t, S, 70.0);
        feed(&mut t, 10 * S, 86.0); // 9 s later: the same one
        assert_eq!(t.list().len(), 1);
        assert_eq!((t.list()[0].end_ms, t.list()[0].peak), (None, 86.0));
        feed(&mut t, 11 * S, 70.0);
        feed(&mut t, 11 * S + MERGE_GAP_MS + S, 85.0); // past the gap: a new one
        assert_eq!(t.list().len(), 2);
        assert_eq!(t.list()[0].end_ms, Some(11 * S));
        assert_eq!(t.list()[1].start_ms, 11 * S + MERGE_GAP_MS + S);
    }

    #[test]
    fn readings_are_tracked_apart_and_below_thresholds_peak_low() {
        let mut t = Tracker::new();
        feed(&mut t, 0, 90.0);
        // a pump: warn at or below 1000 RPM, critical at or below 400
        for (i, v) in [900.0, 350.0, 600.0].into_iter().enumerate() {
            t.observe(i as u64 * S, "board/fan/pump", v, Some(1000.0), Some(400.0), true);
        }
        assert_eq!(t.list().len(), 2);
        let pump = t.list().iter().find(|c| c.id == "board/fan/pump").unwrap();
        assert_eq!((pump.level, pump.peak, pump.below), (CrossingLevel::Crit, 350.0, true));
        // no thresholds: never a crossing
        t.observe(0, "cpu/usage/total", 100.0, None, None, false);
        assert_eq!(t.list().len(), 2);
    }

    #[test]
    fn old_closed_crossings_are_forgotten_and_the_list_is_capped() {
        let mut t = Tracker::new();
        feed(&mut t, 0, 85.0);
        feed(&mut t, S, 70.0);
        feed(&mut t, KEEP_CLOSED_MS + 2 * S, 85.0); // a new one while the old one ages out
        t.prune(KEEP_CLOSED_MS + 2 * S);
        assert_eq!(t.list().len(), 1);
        assert_eq!(t.list()[0].start_ms, KEEP_CLOSED_MS + 2 * S);
        let mut t = Tracker::new();
        for n in 0..(MAX_KEPT as u64 + 20) {
            let id = format!("r{n}");
            t.observe(n * 100 * S, &id, 90.0, Some(80.0), None, false);
            t.observe(n * 100 * S + S, &id, 10.0, Some(80.0), None, false);
        }
        t.prune((MAX_KEPT as u64 + 20) * 100 * S);
        assert_eq!(t.list().len(), MAX_KEPT);
        assert_eq!(t.list()[0].id, "r20", "the oldest went first");
    }

    #[test]
    fn an_undone_reset_puts_the_closed_crossings_back_beside_the_new_ones() {
        let mut t = Tracker::new();
        feed(&mut t, 0, 85.0);
        feed(&mut t, S, 70.0);
        t.observe(2 * S, "gpu0/temp/core", 90.0, Some(80.0), Some(88.0), false);
        let before = t.list().to_vec();
        t.clear_closed();
        // after the reset: the open one goes on, a new one starts
        t.observe(3 * S, "gpu0/temp/core", 92.0, Some(80.0), Some(88.0), false);
        feed(&mut t, 4 * S, 86.0);
        t.absorb_earlier(before);
        let ids: Vec<_> = t.list().iter().map(|c| (c.id.as_str(), c.start_ms, c.peak)).collect();
        assert_eq!(ids, [("cpu/temp/tctl", 0, 85.0), ("gpu0/temp/core", 2 * S, 92.0), ("cpu/temp/tctl", 4 * S, 86.0)]);
    }

    #[test]
    fn reset_drops_what_is_over_and_keeps_what_is_going() {
        let mut t = Tracker::new();
        feed(&mut t, 0, 85.0);
        feed(&mut t, S, 70.0);
        t.observe(2 * S, "gpu0/temp/core", 90.0, Some(80.0), Some(88.0), false);
        t.clear_closed();
        assert_eq!(t.list().len(), 1);
        assert_eq!(t.list()[0].id, "gpu0/temp/core");
    }
}
