//! The limits the window shows. The collector decides them (the defaults, then `thresholds.json`)
//! and sends the ones in force in the readings and in `Snapshot::limit_overrides`; the window only
//! asks for changes and never writes the file.

use crate::theme::Level;
use hwglance_proto::{ClientMsg, Reading, Snapshot};
use std::collections::HashMap;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Thr {
    pub warn: Option<f64>,
    pub crit: Option<f64>,
    /// Alert when the value is at or below the thresholds instead of at or above.
    pub below: bool,
}

impl Thr {
    pub fn is_none(&self) -> bool {
        self.warn.is_none() && self.crit.is_none()
    }
}

/// The limits in force for a reading: the collector's defaults, or the user's replacement of them
/// as a whole (which is what its warn and crit already hold).
pub fn effective(snap: &Snapshot, r: &Reading) -> Thr {
    Thr { warn: r.warn, crit: r.crit, below: snap.limit_overrides.get(&r.id).is_some_and(|o| o.below) }
}

/// The collector's defaults for a reading (always "above").
pub fn default_thr(snap: &Snapshot, r: &Reading) -> Thr {
    match snap.limit_overrides.get(&r.id) {
        Some(o) => Thr { warn: o.default_warn, crit: o.default_crit, below: false },
        None => Thr { warn: r.warn, crit: r.crit, below: false },
    }
}

/// A change of limits the window asks the collector for.
#[derive(Clone, Debug, PartialEq)]
pub enum ThrEdit {
    Set { id: String, thr: Thr },
    Reset { id: String },
}

impl ThrEdit {
    pub fn id(&self) -> &str {
        match self {
            ThrEdit::Set { id, .. } | ThrEdit::Reset { id } => id,
        }
    }

    pub fn message(&self) -> ClientMsg {
        match self {
            ThrEdit::Set { id, thr } => ClientMsg::SetThreshold { id: id.clone(), warn: thr.warn, crit: thr.crit, below: thr.below },
            ThrEdit::Reset { id } => ClientMsg::ResetThreshold { id: id.clone() },
        }
    }
}

/// How long a sent change is shown from here before the snapshots are believed again: long enough
/// for the collector to apply it at its next sample and for that snapshot to arrive.
fn grace(snap: Option<&Snapshot>) -> Duration {
    let interval = snap.map_or(1000, |s| u64::from(s.interval_ms));
    Duration::from_millis(interval * 2 + 1000).max(Duration::from_secs(3))
}

/// Changes sent to the collector that the snapshots may not show yet, so a field being dragged or
/// typed in keeps its value instead of jumping back until the next sample.
#[derive(Default)]
pub struct Pending {
    sent: HashMap<String, (ThrEdit, Instant)>,
}

impl Pending {
    pub fn record(&mut self, edit: ThrEdit) {
        self.sent.insert(edit.id().to_string(), (edit, Instant::now()));
    }

    /// The limits to show for a reading: a change just sent, else those in force.
    pub fn thr(&self, snap: &Snapshot, r: &Reading) -> Thr {
        match self.sent.get(&r.id) {
            Some((ThrEdit::Set { thr, .. }, _)) => *thr,
            Some((ThrEdit::Reset { .. }, _)) => default_thr(snap, r),
            None => effective(snap, r),
        }
    }

    /// Whether the reading's limits are the user's rather than the defaults.
    pub fn custom(&self, snap: &Snapshot, r: &Reading) -> bool {
        match self.sent.get(&r.id) {
            Some((ThrEdit::Set { thr, .. }, _)) => *thr != default_thr(snap, r),
            Some((ThrEdit::Reset { .. }, _)) => false,
            None => snap.limit_overrides.contains_key(&r.id),
        }
    }

    /// The readings with limits of the user's, in id order.
    pub fn custom_ids(&self, snap: &Snapshot) -> Vec<String> {
        let mut ids: Vec<String> = snap.limit_overrides.keys().filter(|id| !matches!(self.sent.get(*id), Some((ThrEdit::Reset { .. }, _)))).cloned().collect();
        for (id, (edit, _)) in &self.sent {
            if matches!(edit, ThrEdit::Set { .. }) && !ids.contains(id) {
                ids.push(id.clone());
            }
        }
        ids.sort();
        ids
    }

    /// Forget the changes the snapshots have had time to show.
    pub fn expire(&mut self, snap: Option<&Snapshot>) {
        let grace = grace(snap);
        self.sent.retain(|_, (_, at)| at.elapsed() < grace);
    }

    pub fn is_empty(&self) -> bool {
        self.sent.is_empty()
    }
}

/// Normal below `warn`, warn from `warn`, crit from `crit`; with `below` the direction flips.
pub fn level_of(v: f64, t: &Thr) -> Level {
    let hit = |limit: f64| if t.below { v <= limit } else { v >= limit };
    if t.crit.is_some_and(hit) {
        Level::Crit
    } else if t.warn.is_some_and(hit) {
        Level::Warn
    } else {
        Level::Normal
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hwglance_proto::{LimitOverride, Unit};

    fn reading(warn: Option<f64>, crit: Option<f64>) -> Reading {
        Reading {
            id: "x".into(),
            label: "X".into(),
            unit: Unit::Celsius,
            value: Some(1.0),
            stats: None,
            note: None,
            desc: None,
            events: None,
            warn,
            crit,
        }
    }

    /// A snapshot whose collector says `x` is customised: the reading carries the limits in force.
    fn snapshot_with(over: Option<LimitOverride>) -> Snapshot {
        let mut s = crate::demo::Demo::new().snapshot(0.0);
        s.limit_overrides.clear();
        if let Some(o) = over {
            s.limit_overrides.insert("x".into(), o);
        }
        s
    }

    #[test]
    fn above_thresholds() {
        let t = Thr { warn: Some(80.0), crit: Some(90.0), below: false };
        assert_eq!(level_of(79.9, &t), Level::Normal);
        assert_eq!(level_of(80.0, &t), Level::Warn);
        assert_eq!(level_of(89.9, &t), Level::Warn);
        assert_eq!(level_of(90.0, &t), Level::Crit);
        assert_eq!(level_of(500.0, &Thr::default()), Level::Normal);
        assert_eq!(level_of(95.0, &Thr { warn: None, crit: Some(90.0), below: false }), Level::Crit);
    }

    #[test]
    fn below_flips_the_direction() {
        // a pump: warn under 1000 RPM, critical under 400 RPM
        let t = Thr { warn: Some(1000.0), crit: Some(400.0), below: true };
        assert_eq!(level_of(2600.0, &t), Level::Normal);
        assert_eq!(level_of(1000.0, &t), Level::Warn);
        assert_eq!(level_of(700.0, &t), Level::Warn);
        assert_eq!(level_of(400.0, &t), Level::Crit);
        assert_eq!(level_of(0.0, &t), Level::Crit);
    }

    #[test]
    fn the_collectors_limits_replace_the_defaults_as_a_whole() {
        // not customised: the reading's own limits are the defaults
        let snap = snapshot_with(None);
        let r = reading(Some(80.0), Some(89.0));
        assert_eq!(effective(&snap, &r), Thr { warn: Some(80.0), crit: Some(89.0), below: false });
        assert_eq!(default_thr(&snap, &r), effective(&snap, &r));
        // customised: the reading carries the user's limits (no warning level now) and the snapshot
        // remembers the defaults and the direction
        let snap = snapshot_with(Some(LimitOverride { warn: None, crit: Some(85.0), below: false, default_warn: Some(80.0), default_crit: Some(89.0) }));
        let r = reading(None, Some(85.0));
        let e = effective(&snap, &r);
        assert_eq!(e, Thr { warn: None, crit: Some(85.0), below: false });
        assert_eq!(level_of(82.0, &e), Level::Normal, "the default warning level is gone, not merged");
        assert_eq!(default_thr(&snap, &r), Thr { warn: Some(80.0), crit: Some(89.0), below: false });
        // a reading without defaults that got limits, and a direction
        let snap = snapshot_with(Some(LimitOverride { warn: Some(2.0), crit: Some(1.0), below: true, default_warn: None, default_crit: None }));
        let r = reading(Some(2.0), Some(1.0));
        assert_eq!(level_of(1.5, &effective(&snap, &r)), Level::Warn);
        assert!(default_thr(&snap, &r).is_none());
    }

    #[test]
    fn a_change_just_sent_shows_until_the_snapshots_catch_up() {
        let snap = snapshot_with(None);
        let r = reading(Some(80.0), Some(89.0));
        let mut p = Pending::default();
        assert_eq!(p.thr(&snap, &r), effective(&snap, &r));
        assert!(!p.custom(&snap, &r));
        let mine = Thr { warn: Some(70.0), crit: None, below: true };
        p.record(ThrEdit::Set { id: "x".into(), thr: mine });
        assert_eq!(p.thr(&snap, &r), mine);
        assert!(p.custom(&snap, &r));
        assert_eq!(p.custom_ids(&snap), vec!["x".to_string()], "listed at once, before the snapshot says so");
        p.record(ThrEdit::Reset { id: "x".into() });
        assert_eq!(p.thr(&snap, &r), Thr { warn: Some(80.0), crit: Some(89.0), below: false });
        assert!(!p.custom(&snap, &r));
        assert!(p.custom_ids(&snap).is_empty());
        // setting the defaults back by hand is not custom either
        p.record(ThrEdit::Set { id: "x".into(), thr: Thr { warn: Some(80.0), crit: Some(89.0), below: false } });
        assert!(!p.custom(&snap, &r));
        // a reset hides the override a snapshot still carries
        let snap = snapshot_with(Some(LimitOverride { warn: Some(1.0), crit: None, below: false, default_warn: Some(80.0), default_crit: Some(89.0) }));
        assert_eq!(p.custom_ids(&snap), vec!["x".to_string()]);
        p.record(ThrEdit::Reset { id: "x".into() });
        assert!(p.custom_ids(&snap).is_empty());
        // and the grace runs out
        p.sent.get_mut("x").unwrap().1 = Instant::now() - Duration::from_secs(10);
        p.expire(Some(&snap));
        assert!(p.is_empty());
    }

    #[test]
    fn edits_become_the_protocol_requests() {
        let set = ThrEdit::Set { id: "fan/pump".into(), thr: Thr { warn: Some(1000.0), crit: Some(400.0), below: true } };
        assert_eq!(set.message(), ClientMsg::SetThreshold { id: "fan/pump".into(), warn: Some(1000.0), crit: Some(400.0), below: true });
        assert_eq!(ThrEdit::Reset { id: "fan/pump".into() }.message(), ClientMsg::ResetThreshold { id: "fan/pump".into() });
    }

    #[test]
    fn the_grace_follows_the_sample_interval() {
        assert_eq!(grace(None), Duration::from_secs(3));
        let mut s = snapshot_with(None);
        s.interval_ms = 5000;
        assert_eq!(grace(Some(&s)), Duration::from_secs(11));
    }
}
