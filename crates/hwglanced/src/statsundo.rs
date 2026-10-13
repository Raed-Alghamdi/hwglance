//! What a stats reset cleared, kept for [`UNDO_RESET_WINDOW_S`] seconds so that a window's Undo
//! can bring it back.
//!
//! Only the latest reset is kept, whichever client asked for it, so an undo always restores the
//! state before the latest reset. The clock is the moment the collector received each request
//! (not when the sampling loop got to it), so a slow sample interval cannot eat into a window.
//! What was recorded after the reset is folded into the restored statistics, not thrown away.

use hwglance_proto::crossings::Tracker;
use hwglance_proto::{Crossing, FlagEvents, Stats, UNDO_RESET_WINDOW_S};
use std::collections::HashMap;
use std::time::{Duration, Instant};

pub const WINDOW: Duration = Duration::from_secs(UNDO_RESET_WINDOW_S);

/// The state a reset clears.
pub struct Backup {
    at: Instant,
    stats: HashMap<String, Stats>,
    events: HashMap<String, FlagEvents>,
    crossings: Vec<Crossing>,
    since_ms: u64,
}

#[derive(Default)]
pub struct Undo {
    backup: Option<Backup>,
}

impl Undo {
    /// Keep the state about to be cleared (replacing any earlier backup).
    pub fn keep(
        &mut self,
        at: Instant,
        stats: &HashMap<String, Stats>,
        flags: &HashMap<String, (bool, FlagEvents)>,
        crossings: &Tracker,
        since_ms: u64,
    ) {
        self.backup = Some(Backup {
            at,
            stats: stats.clone(),
            events: flags.iter().map(|(id, (_, ev))| (id.clone(), *ev)).collect(),
            crossings: crossings.list().to_vec(),
            since_ms,
        });
    }

    /// Put the kept state back, if there is one and `at` is within the window of its reset.
    /// Returns whether it did. The backup is spent either way.
    pub fn restore(
        &mut self,
        at: Instant,
        stats: &mut HashMap<String, Stats>,
        flags: &mut HashMap<String, (bool, FlagEvents)>,
        crossings: &mut Tracker,
        since_ms: &mut u64,
    ) -> bool {
        let Some(b) = self.backup.take() else { return false };
        if at.saturating_duration_since(b.at) > WINDOW {
            return false;
        }
        for (id, earlier) in &b.stats {
            match stats.get_mut(id) {
                Some(now) => now.absorb_earlier(earlier),
                None => {
                    stats.insert(id.clone(), *earlier);
                }
            }
        }
        for (id, earlier) in &b.events {
            if let Some((_, ev)) = flags.get_mut(id) {
                ev.absorb_earlier(earlier);
            }
        }
        crossings.absorb_earlier(b.crossings);
        *since_ms = b.since_ms;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hwglance_proto::CrossingLevel;

    struct State {
        stats: HashMap<String, Stats>,
        flags: HashMap<String, (bool, FlagEvents)>,
        crossings: Tracker,
        since: u64,
    }

    impl State {
        fn before() -> State {
            let mut stats = HashMap::new();
            let mut s = Stats::first(50.0);
            s.push(70.0);
            stats.insert("cpu/temp/tctl".to_string(), s);
            let mut flags = HashMap::new();
            flags.insert("cpu/throttle".to_string(), (false, FlagEvents { count: 2, last_unix_ms: 5_000, ended_unix_ms: Some(6_000) }));
            let mut crossings = Tracker::new();
            crossings.observe(1_000, "cpu/temp/tctl", 85.0, Some(80.0), Some(90.0), false);
            crossings.observe(2_000, "cpu/temp/tctl", 60.0, Some(80.0), Some(90.0), false);
            State { stats, flags, crossings, since: 100 }
        }

        /// What `Core::reset_stats` does after keeping a backup.
        fn clear(&mut self) {
            self.stats.clear();
            for (_, ev) in self.flags.values_mut() {
                *ev = FlagEvents { count: 0, last_unix_ms: 0, ended_unix_ms: None };
            }
            self.crossings.clear_closed();
            self.since = 9_000;
        }

        fn keep(&self, u: &mut Undo, at: Instant) {
            u.keep(at, &self.stats, &self.flags, &self.crossings, self.since);
        }

        fn restore(&mut self, u: &mut Undo, at: Instant) -> bool {
            u.restore(at, &mut self.stats, &mut self.flags, &mut self.crossings, &mut self.since)
        }
    }

    #[test]
    fn an_undo_within_the_window_restores_exactly_what_the_reset_cleared() {
        let t0 = Instant::now();
        let mut st = State::before();
        let mut u = Undo::default();
        st.keep(&mut u, t0);
        st.clear();
        assert!(st.stats.is_empty() && st.crossings.list().is_empty());
        assert!(st.restore(&mut u, t0 + Duration::from_secs(30)), "30 s exactly is still inside");
        let again = State::before();
        assert_eq!(st.stats, again.stats);
        assert_eq!(st.flags, again.flags);
        assert_eq!(st.crossings.list(), again.crossings.list());
        assert_eq!(st.since, 100);
        assert!(!st.restore(&mut u, t0 + Duration::from_secs(31)), "an undo is spent once used");
    }

    #[test]
    fn samples_taken_after_the_reset_are_kept() {
        let t0 = Instant::now();
        let mut st = State::before();
        let mut u = Undo::default();
        st.keep(&mut u, t0);
        st.clear();
        st.stats.insert("cpu/temp/tctl".into(), Stats::first(95.0));
        st.flags.get_mut("cpu/throttle").unwrap().1 = FlagEvents { count: 1, last_unix_ms: 8_000, ended_unix_ms: None };
        st.crossings.observe(8_500, "cpu/temp/tctl", 95.0, Some(80.0), Some(90.0), false);
        assert!(st.restore(&mut u, t0 + Duration::from_secs(10)));
        let s = st.stats["cpu/temp/tctl"];
        assert_eq!((s.min, s.max, s.samples), (50.0, 95.0, 3));
        assert_eq!(st.flags["cpu/throttle"].1, FlagEvents { count: 3, last_unix_ms: 8_000, ended_unix_ms: None });
        let c: Vec<_> = st.crossings.list().iter().map(|c| (c.start_ms, c.level)).collect();
        assert_eq!(c, [(1_000, CrossingLevel::Warn), (8_500, CrossingLevel::Crit)]);
    }

    #[test]
    fn a_late_undo_does_nothing_and_a_newer_reset_replaces_the_backup() {
        let t0 = Instant::now();
        let mut st = State::before();
        let mut u = Undo::default();
        st.keep(&mut u, t0);
        st.clear();
        assert!(!st.restore(&mut u, t0 + Duration::from_secs(31)), "past the window");
        assert!(st.stats.is_empty(), "nothing came back");
        assert_eq!(st.since, 9_000);

        // two resets: the undo goes back to the state before the latest one
        let mut st = State::before();
        let mut u = Undo::default();
        st.keep(&mut u, t0);
        st.clear();
        st.stats.insert("gpu0/temp/core".into(), Stats::first(40.0));
        st.keep(&mut u, t0 + Duration::from_secs(5));
        st.stats.clear();
        st.since = 9_500;
        assert!(!st.restore(&mut u, t0 + Duration::from_secs(36)), "30 s count from the latest reset");
        st.keep(&mut u, t0 + Duration::from_secs(40));
        assert!(st.restore(&mut u, t0 + Duration::from_secs(41)));
        assert!(st.stats.is_empty(), "the state kept at the second keep was empty");
        assert!(!Undo::default().restore(t0, &mut st.stats, &mut st.flags, &mut st.crossings, &mut st.since), "nothing kept");
    }
}
