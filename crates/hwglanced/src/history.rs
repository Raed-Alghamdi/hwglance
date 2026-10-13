//! Recent values of every reading, kept in memory for the window's hover graphs.
//!
//! One ring per reading id, 8 bytes per point (u32 milliseconds since a base time, f32 value),
//! trimmed by time.

use hwglance_proto::{HistorySeries, Unit};
use std::collections::{HashMap, VecDeque};

/// Most points returned per series; longer ranges are averaged down to this.
pub const MAX_POINTS: usize = 900;
/// Most series answered in one request.
pub const MAX_IDS: usize = 512;
/// Offsets are rebased before they come near the end of a u32 (about 49 days).
const REBASE_AT_MS: u64 = 3_000_000_000;

struct Series {
    unit: Unit,
    points: VecDeque<(u32, f32)>,
}

pub struct History {
    /// Unix time (ms) that offset 0 corresponds to.
    base_ms: u64,
    window_ms: u64,
    series: HashMap<String, Series>,
    pushes: u64,
}

impl History {
    pub fn new(now_ms: u64, minutes: u32) -> Self {
        History {
            base_ms: now_ms,
            window_ms: u64::from(minutes) * 60_000,
            series: HashMap::new(),
            pushes: 0,
        }
    }

    pub fn set_minutes(&mut self, minutes: u32) {
        self.window_ms = u64::from(minutes) * 60_000;
    }

    /// Record one value; older points than the window are dropped from this series.
    pub fn record(&mut self, now_ms: u64, id: &str, unit: Unit, value: f64) {
        if !value.is_finite() {
            return;
        }
        if now_ms < self.base_ms {
            // the clock went back past where the rings begin: that history belongs to a timeline
            // that no longer exists
            self.series.clear();
            self.base_ms = now_ms;
        }
        let off = now_ms - self.base_ms;
        if off > REBASE_AT_MS {
            self.rebase(now_ms);
        }
        let off = (now_ms - self.base_ms) as u32;
        let keep_from = (now_ms.saturating_sub(self.window_ms)).saturating_sub(self.base_ms) as u32;
        let s = match self.series.get_mut(id) {
            Some(s) => s,
            None => self.series.entry(id.to_string()).or_insert(Series {
                unit,
                points: VecDeque::new(),
            }),
        };
        // A clock stepped back (an NTP correction after suspend) leaves points stamped later than
        // now. Drop them so the ring stays in time order: the search for the first point in range
        // and the averaging both rely on that.
        while s.points.back().is_some_and(|p| p.0 > off) {
            s.points.pop_back();
        }
        s.points.push_back((off, value as f32));
        while s.points.front().is_some_and(|p| p.0 < keep_from) {
            s.points.pop_front();
        }
        // A ring that grew for a long window and then shrank gives its memory back.
        if s.points.capacity() > 4 * s.points.len().max(16) {
            s.points.shrink_to(2 * s.points.len().max(16));
        }
        self.pushes += 1;
    }

    /// Drop series that have no points left (readings that went away).
    pub fn sweep(&mut self, now_ms: u64) {
        let keep_from = now_ms
            .saturating_sub(self.window_ms)
            .saturating_sub(self.base_ms) as u32;
        self.series
            .retain(|_, s| s.points.back().is_some_and(|p| p.0 >= keep_from));
    }

    fn rebase(&mut self, now_ms: u64) {
        // Everything stored is within the window (at most 4 h), so one day before now is safe.
        let new_base = now_ms.saturating_sub(86_400_000).max(self.base_ms);
        // the shift can be longer than a u32 holds (a resume after weeks); points older than it
        // end at offset 0 and leave with the window
        let shift = new_base - self.base_ms;
        for s in self.series.values_mut() {
            for p in s.points.iter_mut() {
                p.0 = u64::from(p.0).saturating_sub(shift) as u32;
            }
        }
        self.base_ms = new_base;
    }

    /// The last `seconds` of each requested id that has data, averaged to at most
    /// [`MAX_POINTS`] points per series.
    pub fn query(&self, now_ms: u64, ids: &[String], seconds: u32) -> Vec<HistorySeries> {
        let span_ms = (u64::from(seconds) * 1000).min(self.window_ms).max(1000);
        let from = now_ms.saturating_sub(span_ms);
        let bucket_ms = span_ms.div_ceil(MAX_POINTS as u64).max(1);
        let mut out = Vec::new();
        for id in ids.iter().take(MAX_IDS) {
            let Some(s) = self.series.get(id) else {
                continue;
            };
            let mut points: Vec<(u64, f32)> = Vec::new();
            // (bucket index, sum of times, sum of values, count)
            let mut acc: Option<(u64, f64, f64, u32)> = None;
            let flush = |a: (u64, f64, f64, u32), points: &mut Vec<(u64, f32)>| {
                points.push(((a.1 / f64::from(a.3)) as u64, (a.2 / f64::from(a.3)) as f32));
            };
            // Points are oldest first; binary search for the first one in range.
            let start = s
                .points
                .partition_point(|p| self.base_ms + u64::from(p.0) < from);
            for &(t, v) in s.points.iter().skip(start) {
                let abs = self.base_ms + u64::from(t);
                let b = (abs.saturating_sub(from) / bucket_ms).min(MAX_POINTS as u64 - 1);
                match &mut acc {
                    Some(a) if a.0 == b => {
                        a.1 += abs as f64;
                        a.2 += f64::from(v);
                        a.3 += 1;
                    }
                    _ => {
                        if let Some(a) = acc.take() {
                            flush(a, &mut points);
                        }
                        acc = Some((b, abs as f64, f64::from(v), 1));
                    }
                }
            }
            if let Some(a) = acc {
                flush(a, &mut points);
            }
            if !points.is_empty() {
                out.push(HistorySeries {
                    id: id.clone(),
                    unit: s.unit,
                    points,
                });
            }
        }
        out
    }

    #[cfg(test)]
    fn len(&self, id: &str) -> usize {
        self.series.get(id).map_or(0, |s| s.points.len())
    }

    /// Total points stored (for the cost benchmark).
    pub fn total_points(&self) -> usize {
        self.series.values().map(|s| s.points.len()).sum()
    }

    pub fn series_count(&self) -> usize {
        self.series.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trims_by_time() {
        let mut h = History::new(0, 1); // one minute
        for s in 0..180u64 {
            h.record(s * 1000, "a", Unit::Percent, s as f64);
        }
        // points from the last 60 s (inclusive of the boundary second)
        assert!(h.len("a") <= 61 && h.len("a") >= 60, "{}", h.len("a"));
    }

    #[test]
    fn query_range_and_downsampling() {
        let mut h = History::new(0, 240);
        for s in 0..7200u64 {
            h.record(1_000_000 + s * 1000, "a", Unit::Celsius, (s % 100) as f64);
        }
        let now = 1_000_000 + 7199 * 1000;
        let r = h.query(now, &["a".into(), "missing".into()], 300);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].points.len(), 301.min(MAX_POINTS));
        assert!(r[0].points.windows(2).all(|w| w[0].0 < w[1].0));
        let long = h.query(now, &["a".into()], 7200);
        assert!(long[0].points.len() <= MAX_POINTS);
        assert!(long[0].points.len() > 800);
        // averaging keeps the mean (values cycle 0..99, mean 49.5)
        let mean: f32 =
            long[0].points.iter().map(|p| p.1).sum::<f32>() / long[0].points.len() as f32;
        assert!((mean - 49.5).abs() < 2.0, "{mean}");
    }

    #[test]
    fn gaps_stay_gaps_and_sweep_drops_dead_series() {
        let mut h = History::new(0, 1);
        h.record(1000, "a", Unit::Rpm, 5.0);
        h.record(50_000, "a", Unit::Rpm, 6.0);
        let r = h.query(50_000, &["a".into()], 60);
        assert_eq!(r[0].points.len(), 2);
        h.sweep(500_000);
        assert_eq!(h.series_count(), 0);
    }

    #[test]
    fn a_clock_stepped_back_keeps_the_ring_in_order() {
        let mut h = History::new(0, 30);
        for s in 0..10u64 {
            h.record(10_000_000 + s * 1000, "a", Unit::Celsius, 50.0 + s as f64);
        }
        // an hour back: the ten points stamped "later" are dropped, the new ones follow in order
        let back = 10_000_000 - 3_600_000;
        for s in 0..5u64 {
            h.record(back + s * 1000, "a", Unit::Celsius, 70.0 + s as f64);
        }
        let r = h.query(back + 4000, &["a".into()], 600);
        assert_eq!(r.len(), 1);
        let pts = &r[0].points;
        assert!(pts.windows(2).all(|w| w[0].0 < w[1].0), "time order: {pts:?}");
        assert!(pts.iter().all(|p| p.0 <= back + 4000), "nothing from the future");
        assert_eq!(pts.last().map(|p| p.1), Some(74.0));
    }

    #[test]
    fn a_clock_stepped_back_before_the_start_begins_again() {
        let mut h = History::new(5_000_000, 30);
        h.record(5_010_000, "a", Unit::Rpm, 1.0);
        h.record(1_000, "a", Unit::Rpm, 2.0);
        let r = h.query(1_000, &["a".into()], 60);
        assert_eq!(r[0].points, vec![(1_000, 2.0)]);
    }

    #[test]
    fn a_resume_after_weeks_does_not_resurrect_old_points() {
        let mut h = History::new(0, 30);
        h.record(1000, "a", Unit::Rpm, 5.0);
        // 60 days later: the shift of the rebase does not fit a u32
        let late = 60 * 86_400_000u64;
        h.record(late, "a", Unit::Rpm, 7.0);
        h.record(late + 1000, "a", Unit::Rpm, 8.0);
        let r = h.query(late + 1000, &["a".into()], 1800);
        // only the two new values, averaged into one bucket; the one from 60 days ago is gone
        assert_eq!(r[0].points.len(), 1);
        assert!(r[0].points[0].0 >= late && (r[0].points[0].1 - 7.5).abs() < 1e-6, "{:?}", r[0].points);
    }

    #[test]
    fn rebase_keeps_values() {
        let mut h = History::new(0, 1);
        h.record(1000, "a", Unit::Rpm, 5.0);
        let late = REBASE_AT_MS + 10_000;
        h.record(late, "a", Unit::Rpm, 7.0);
        let r = h.query(late, &["a".into()], 60);
        assert_eq!(r[0].points, vec![(late, 7.0)]);
    }
}
