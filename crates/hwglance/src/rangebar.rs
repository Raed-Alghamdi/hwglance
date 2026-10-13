//! The reference-range bar of a sensor row: observed min to max, the average tick, the current
//! marker and the warning and critical zones, all on one track. Geometry is pure and tested;
//! `paint` draws it.

use crate::theme::{Level, Palette};
use crate::thresholds::{level_of, Thr};
use egui::{pos2, vec2, Color32, CornerRadius, Painter, Rect, Stroke};
use hwglance_proto::{Reading, Snapshot, Unit};

/// Value range the track covers.
pub fn domain(unit: Unit, obs_min: f64, obs_max: f64, thr: &Thr) -> (f64, f64) {
    domain_in(unit, obs_min, obs_max, thr, None)
}

/// Value range the track covers, with the reading's known capacity when something states it
/// (total RAM, total VRAM, the GPU power limit): the track then runs from 0 to the capacity, so
/// "Memory used" reads as a share of the memory and not as a position against its own peak.
/// Percent and temperature scales ignore it.
pub fn domain_in(unit: Unit, obs_min: f64, obs_max: f64, thr: &Thr, cap: Option<f64>) -> (f64, f64) {
    let (lo, hi) = match unit {
        Unit::Percent => (0.0, 100.0),
        Unit::Celsius => {
            let lo = (obs_min.min(25.0) / 5.0).floor() * 5.0;
            let top = obs_max.max(thr.crit.or(thr.warn).unwrap_or(obs_max));
            (lo, top * 1.05)
        }
        _ => match cap {
            Some(c) => (0.0, c.max(obs_max).max(thr.crit.unwrap_or(0.0))),
            None => (0.0, obs_max.max(thr.crit.unwrap_or(obs_max)) * 1.05),
        },
    };
    if hi <= lo { (lo, lo + 1.0) } else { (lo, hi) }
}

/// The largest value a reading can have, when the snapshot says so: total RAM for the memory
/// amounts, the swap size, total VRAM, and the power limit for the GPU's power draw. Nothing
/// states a fan's maximum speed or a drive's top transfer rate, so those have none.
pub fn capacity(snap: &Snapshot, r: &Reading) -> Option<f64> {
    let s = &snap.summary;
    let cap = match r.id.as_str() {
        "memory/usage/used" | "memory/usage/available" | "memory/usage/cached" | "memory/usage/cache" => s.ram_total_bytes,
        "memory/usage/swap" => s.swap_total_bytes,
        "gpu0/mem/used" => s.gpu_vram_total_bytes,
        "gpu0/power/draw" => snap
            .sensors
            .iter()
            .flat_map(|g| &g.sections)
            .flat_map(|sec| &sec.readings)
            .find(|x| x.id == "gpu0/power/limit")
            .and_then(|x| x.value),
        _ => None,
    };
    cap.filter(|c| *c > 0.0)
}

/// Readings that hold one value for as long as the machine runs (a configured memory speed, the
/// GPU's power limit): a range bar would only show a marker pinned somewhere, so they get none.
pub fn is_constant(r: &Reading) -> bool {
    r.unit == Unit::Mts || matches!(r.id.as_str(), "memory/speed/clock" | "gpu0/power/limit")
}

/// How far a reading must have moved, as a share of its largest value, before its bar means
/// anything. On a scale that follows the observed maximum, a value that has not moved always sits
/// at the right end.
pub const MIN_SPREAD: f64 = 0.05;

/// An observed range covering at least this much of the track is drawn lighter.
pub const QUIET_COVERAGE: f32 = 0.6;
/// How much of the range alpha a quiet fill keeps.
pub const QUIET_FILL: f32 = 0.5;

/// Everything the painter needs, as fractions of the track (0 = left end, 1 = right end).
#[derive(Clone, Debug, PartialEq)]
pub struct BarGeom {
    pub lo: f64,
    pub hi: f64,
    pub range: Option<(f32, f32)>,
    pub avg: Option<f32>,
    pub marker: Option<f32>,
    pub warn_zone: Option<(f32, f32)>,
    pub crit_zone: Option<(f32, f32)>,
    /// Level of the current value, which colours the marker.
    pub level: Level,
    /// The observed range covers most of the track, so its fill is drawn lighter and attention
    /// goes to the marker, the thresholds and the watched values.
    pub quiet: bool,
}

/// Geometry of a reading's bar on the scale of its observed range, or `None` for flags and
/// constants (they get no bar).
pub fn geometry(r: &Reading, thr: &Thr) -> Option<BarGeom> {
    geometry_in(r, thr, None)
}

/// Geometry of a reading's bar. `cap` is its known capacity (see [`capacity`]). On a scale that
/// follows the observed maximum (no capacity, no thresholds, not a percentage or a temperature)
/// the track stays empty until the value has moved by [`MIN_SPREAD`].
pub fn geometry_in(r: &Reading, thr: &Thr, cap: Option<f64>) -> Option<BarGeom> {
    if r.unit == Unit::Flag || is_constant(r) {
        return None;
    }
    let (obs_min, obs_max) = match (&r.stats, r.value) {
        (Some(s), _) => (s.min, s.max),
        (None, Some(v)) => (v, v),
        (None, None) => (0.0, 0.0),
    };
    let (lo, hi) = domain_in(r.unit, obs_min, obs_max, thr, cap);
    let floating = cap.is_none() && thr.is_none() && !matches!(r.unit, Unit::Percent | Unit::Celsius);
    if floating && (obs_max - obs_min) < MIN_SPREAD * obs_max.abs().max(f64::MIN_POSITIVE) {
        return Some(BarGeom { lo, hi, range: None, avg: None, marker: None, warn_zone: None, crit_zone: None, level: Level::Normal, quiet: false });
    }
    let f = |v: f64| (((v - lo) / (hi - lo)).clamp(0.0, 1.0)) as f32;
    let zone = |limit: Option<f64>| -> Option<(f32, f32)> {
        let l = limit?;
        let x = f(l);
        let z = if thr.below { (0.0, x) } else { (x, 1.0) };
        (z.1 - z.0 > 0.001).then_some(z)
    };
    // The critical zone is nested inside the warning zone: both start at the end of the domain.
    let warn_zone = zone(thr.warn);
    let crit_zone = zone(thr.crit);
    let range = r.stats.as_ref().map(|s| (f(s.min), f(s.max)));
    Some(BarGeom {
        lo,
        hi,
        quiet: range.is_some_and(|(a, b)| b - a >= QUIET_COVERAGE),
        range,
        avg: r.stats.as_ref().map(|s| f(s.avg)),
        marker: r.value.map(f),
        warn_zone,
        crit_zone,
        level: r.value.map(|v| level_of(v, thr)).unwrap_or(Level::Normal),
    })
}

pub const TRACK_H: f32 = 6.0;

/// Share of the device colour in the empty track, over the panel.
pub const TRACK_TINT: f32 = 0.18;

/// The empty track: the panel with a low-strength wash of the device colour, so every row
/// carries its device even where nothing has been observed yet.
pub fn track_color(device: Color32, pal: &Palette) -> Color32 {
    pal.panel.lerp_to_gamma(device, TRACK_TINT)
}

/// Draw the bar into `area` (its vertical centre is the track's).
pub fn paint(p: &Painter, area: Rect, g: &BarGeom, device: Color32, pal: &Palette) {
    let w = area.width();
    if w < 8.0 {
        return;
    }
    let cy = area.center().y;
    let track = Rect::from_min_size(pos2(area.left(), cy - TRACK_H / 2.0), vec2(w, TRACK_H));
    p.rect_filled(track, 3.0, track_color(device, pal));
    let x = |f: f32| track.left() + f * w;
    let zone_rect = |z: (f32, f32), color: Color32| {
        let r = Rect::from_min_max(pos2(x(z.0), track.top()), pos2(x(z.1), track.bottom()));
        let left = if z.0 <= 0.001 { 3 } else { 0 };
        let right = if z.1 >= 0.999 { 3 } else { 0 };
        let cr = CornerRadius { nw: left, sw: left, ne: right, se: right };
        p.rect_filled(r, cr, color);
    };
    if let Some(z) = g.warn_zone {
        zone_rect(z, pal.warn_fill.gamma_multiply(0.28));
    }
    if let Some(z) = g.crit_zone {
        zone_rect(z, pal.crit.gamma_multiply(0.38));
    }
    if let Some((a, b)) = g.range {
        let (x0, x1) = (x(a), x(b).max(x(a) + 3.0));
        p.rect_filled(
            Rect::from_min_max(pos2(x0, track.top()), pos2(x1.min(track.right()), track.bottom())),
            3.0,
            device.gamma_multiply(pal.range_alpha * if g.quiet { QUIET_FILL } else { 1.0 }),
        );
    }
    if let Some(a) = g.avg {
        p.rect_filled(Rect::from_center_size(pos2(x(a), cy), vec2(1.5, 10.0)), 0.0, pal.ink3);
    }
    if let Some(m) = g.marker {
        let color = pal.level_color(g.level).unwrap_or(pal.ink);
        let cx = x(m).clamp(track.left() + 1.5, track.right() - 1.5);
        p.rect(
            Rect::from_center_size(pos2(cx, cy), vec2(3.0, 14.0)),
            1.5,
            color,
            Stroke::new(0.5, pal.panel),
            egui::StrokeKind::Outside,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hwglance_proto::Stats;

    fn reading(unit: Unit, value: f64, min: f64, max: f64, avg: f64) -> Reading {
        Reading {
            id: "x".into(),
            label: "X".into(),
            unit,
            value: Some(value),
            stats: Some(Stats { min, max, avg, samples: 10 }),
            note: None,
            desc: None,
            events: None,
            warn: None,
            crit: None,
        }
    }

    #[test]
    fn the_track_is_a_low_wash_of_the_device_colour_and_the_range_is_stronger() {
        for pal in [Palette::dark(), Palette::light()] {
            let channel_dist = |a: Color32, b: Color32| -> i32 {
                (a.r() as i32 - b.r() as i32).abs() + (a.g() as i32 - b.g() as i32).abs() + (a.b() as i32 - b.b() as i32).abs()
            };
            for device in [pal.cpu, pal.gpu, pal.mem, pal.drive, pal.board] {
                let track = track_color(device, &pal);
                assert_ne!(track, pal.rule, "no longer the grey rules colour");
                // between the panel and the device colour, much closer to the panel
                let (to_panel, to_device) = (channel_dist(track, pal.panel), channel_dist(track, device));
                assert!(to_panel > 0 && to_panel * 3 < to_device, "{to_panel} vs {to_device}");
                // the observed range (device colour at RANGE_ALPHA over the track) is clearly stronger
                let range = track.blend(device.gamma_multiply(pal.range_alpha));
                assert!(channel_dist(range, pal.panel) > 2 * channel_dist(track, pal.panel));
            }
            // each device gets its own hue
            assert_ne!(track_color(pal.cpu, &pal), track_color(pal.gpu, &pal));
        }
    }

    #[test]
    fn percent_domain_is_fixed() {
        assert_eq!(domain(Unit::Percent, 12.0, 40.0, &Thr::default()), (0.0, 100.0));
    }

    #[test]
    fn celsius_domain_floors_to_five_and_includes_the_threshold() {
        let none = Thr::default();
        // observed 41..67: low end is min(41, 25) = 25 -> 25; top is 67 * 1.05
        let (lo, hi) = domain(Unit::Celsius, 41.0, 67.0, &none);
        assert_eq!(lo, 25.0);
        assert!((hi - 67.0 * 1.05).abs() < 1e-9);
        // a cold reading pulls the low end down to a multiple of 5
        assert_eq!(domain(Unit::Celsius, 18.3, 30.0, &none).0, 15.0);
        // the critical threshold stretches the top, warn is used when there is no crit
        let thr = Thr { warn: Some(80.0), crit: Some(95.0), below: false };
        assert!((domain(Unit::Celsius, 40.0, 60.0, &thr).1 - 95.0 * 1.05).abs() < 1e-9);
        let warn_only = Thr { warn: Some(80.0), crit: None, below: false };
        assert!((domain(Unit::Celsius, 40.0, 60.0, &warn_only).1 - 80.0 * 1.05).abs() < 1e-9);
        // observed above the threshold wins
        assert!((domain(Unit::Celsius, 40.0, 99.0, &thr).1 - 99.0 * 1.05).abs() < 1e-9);
    }

    #[test]
    fn other_units_start_at_zero_and_use_crit_only() {
        let thr = Thr { warn: Some(3000.0), crit: Some(4000.0), below: false };
        let (lo, hi) = domain(Unit::Rpm, 900.0, 1500.0, &thr);
        assert_eq!(lo, 0.0);
        assert!((hi - 4000.0 * 1.05).abs() < 1e-9);
        let (_, hi) = domain(Unit::Watt, 20.0, 140.0, &Thr { warn: Some(500.0), crit: None, below: false });
        assert!((hi - 140.0 * 1.05).abs() < 1e-9, "warn alone does not stretch non-temperature domains");
        // degenerate domains stay valid
        let (lo, hi) = domain(Unit::Watt, 0.0, 0.0, &Thr::default());
        assert!(hi > lo);
    }

    #[test]
    fn zones_run_from_the_threshold_to_the_end() {
        let mut r = reading(Unit::Percent, 50.0, 10.0, 80.0, 40.0);
        r.warn = Some(70.0);
        r.crit = Some(90.0);
        let thr = Thr { warn: r.warn, crit: r.crit, below: false };
        let g = geometry(&r, &thr).unwrap();
        assert_eq!(g.warn_zone, Some((0.7, 1.0)));
        assert_eq!(g.crit_zone, Some((0.9, 1.0)));
        assert_eq!(g.range, Some((0.1, 0.8)));
        assert_eq!(g.avg, Some(0.4));
        assert_eq!(g.marker, Some(0.5));
        assert_eq!(g.level, Level::Normal);
    }

    #[test]
    fn zones_mirror_when_below() {
        // a pump: warn at or below 1000 RPM, critical at or below 400 RPM
        let r = reading(Unit::Rpm, 2650.0, 2600.0, 2700.0, 2650.0);
        let thr = Thr { warn: Some(1000.0), crit: Some(400.0), below: true };
        let g = geometry(&r, &thr).unwrap();
        let (_, hi) = domain(Unit::Rpm, 2600.0, 2700.0, &Thr { warn: None, crit: Some(400.0), below: true });
        let wz = g.warn_zone.unwrap();
        let cz = g.crit_zone.unwrap();
        assert_eq!(wz.0, 0.0);
        assert!((wz.1 as f64 - 1000.0 / hi).abs() < 1e-4);
        assert_eq!(cz.0, 0.0);
        assert!(cz.1 < wz.1, "the critical zone is nested inside the warning zone");
        assert_eq!(g.level, Level::Normal);
        // dropping into the zone changes the marker level
        let low = reading(Unit::Rpm, 300.0, 300.0, 2700.0, 2000.0);
        assert_eq!(geometry(&low, &thr).unwrap().level, Level::Crit);
    }

    #[test]
    fn marker_level_follows_thresholds() {
        let mut r = reading(Unit::Celsius, 82.0, 40.0, 82.0, 60.0);
        r.warn = Some(80.0);
        r.crit = Some(90.0);
        let thr = Thr { warn: r.warn, crit: r.crit, below: false };
        assert_eq!(geometry(&r, &thr).unwrap().level, Level::Warn);
        r.value = Some(95.0);
        assert_eq!(geometry(&r, &thr).unwrap().level, Level::Crit);
    }

    #[test]
    fn flags_get_no_bar_and_missing_stats_still_give_a_marker() {
        let mut f = reading(Unit::Flag, 0.0, 0.0, 0.0, 0.0);
        f.stats = None;
        assert!(geometry(&f, &Thr::default()).is_none());
        let mut r = reading(Unit::Percent, 33.0, 0.0, 0.0, 0.0);
        r.stats = None;
        let g = geometry(&r, &Thr::default()).unwrap();
        assert_eq!(g.range, None);
        assert_eq!(g.avg, None);
        assert!(g.marker.is_some());
        r.value = None;
        assert_eq!(geometry(&r, &Thr::default()).unwrap().marker, None);
    }

    fn sized(unit: Unit, id: &str, value: f64, min: f64, max: f64) -> Reading {
        let mut r = reading(unit, value, min, max, (min + max) / 2.0);
        r.id = id.into();
        r
    }

    fn snap_with_capacities() -> Snapshot {
        let mut s = crate::demo::Demo::new().snapshot(0.0);
        s.summary.ram_total_bytes = Some(64.0 * GIB);
        s.summary.swap_total_bytes = Some(8.0 * GIB);
        s.summary.gpu_vram_total_bytes = Some(24.0 * GIB);
        let mut limit = sized(Unit::Watt, "gpu0/power/limit", 450.0, 450.0, 450.0);
        limit.value = Some(450.0);
        s.sensors[0].sections[0].readings.push(limit);
        s
    }

    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

    #[test]
    fn known_capacities_are_the_scale() {
        let s = snap_with_capacities();
        let cap = |id: &str, unit| capacity(&s, &sized(unit, id, 1.0, 0.0, 1.0));
        assert_eq!(cap("memory/usage/used", Unit::Bytes), Some(64.0 * GIB));
        assert_eq!(cap("memory/usage/available", Unit::Bytes), Some(64.0 * GIB));
        assert_eq!(cap("memory/usage/swap", Unit::Bytes), Some(8.0 * GIB));
        assert_eq!(cap("gpu0/mem/used", Unit::Bytes), Some(24.0 * GIB));
        assert_eq!(cap("gpu0/power/draw", Unit::Watt), Some(450.0), "the power limit row");
        assert_eq!(cap("board/fan/nct6799/fan1", Unit::Rpm), None, "nothing states a fan's maximum");
        assert_eq!(cap("disk/x/io/read", Unit::BytesPerSec), None, "nor a drive's top rate");

        // Memory used 25 GiB of 64 sits at 39 %, not at the right end of its own peak
        let used = sized(Unit::Bytes, "memory/usage/used", 25.0 * GIB, 20.0 * GIB, 25.3 * GIB);
        let g = geometry_in(&used, &Thr::default(), cap("memory/usage/used", Unit::Bytes)).unwrap();
        assert!((g.marker.unwrap() as f64 - 25.0 / 64.0).abs() < 1e-3, "{:?}", g.marker);
        assert_eq!(g.hi, 64.0 * GIB);
        // the same row without a capacity (an older collector) follows its peak and hugs the end
        let g = geometry_in(&used, &Thr::default(), None).unwrap();
        assert!(g.marker.unwrap() > 0.9);
        // GPU power 63.5 W against a 450 W limit
        let power = sized(Unit::Watt, "gpu0/power/draw", 63.5, 30.0, 140.0);
        let g = geometry_in(&power, &Thr::default(), Some(450.0)).unwrap();
        assert!((g.marker.unwrap() - 63.5 / 450.0).abs() < 1e-3);
        // a value past its capacity stretches the scale rather than leaving the track
        let over = sized(Unit::Watt, "gpu0/power/draw", 500.0, 30.0, 500.0);
        assert_eq!(geometry_in(&over, &Thr::default(), Some(450.0)).unwrap().marker, Some(1.0));
        // percentages and temperatures ignore a capacity
        assert_eq!(domain_in(Unit::Percent, 0.0, 10.0, &Thr::default(), Some(5.0)), (0.0, 100.0));
        assert_eq!(domain_in(Unit::Celsius, 40.0, 60.0, &Thr::default(), Some(5.0)).0, 25.0);
    }

    #[test]
    fn constants_get_no_bar() {
        for (unit, id) in [(Unit::Mts, "memory/speed/mts"), (Unit::Mhz, "memory/speed/clock"), (Unit::Watt, "gpu0/power/limit")] {
            let r = sized(unit, id, 6000.0, 6000.0, 6000.0);
            assert!(is_constant(&r) && geometry_in(&r, &Thr::default(), None).is_none(), "{id}");
        }
        let clock = sized(Unit::Mhz, "cpu/clock/avg", 4200.0, 3000.0, 5000.0);
        assert!(!is_constant(&clock));
    }

    #[test]
    fn a_bar_stays_empty_until_the_value_has_spread() {
        // a voltage that has not moved: the track is there, the range, tick and marker are not
        let flat = sized(Unit::Volt, "gpu0/power/voltage", 0.87, 0.87, 0.87);
        let g = geometry_in(&flat, &Thr::default(), None).unwrap();
        assert_eq!((g.range, g.avg, g.marker), (None, None, None));
        let barely = sized(Unit::Volt, "gpu0/power/voltage", 0.87, 0.86, 0.87);
        assert!(geometry_in(&barely, &Thr::default(), None).unwrap().marker.is_none(), "1 % is not a spread");
        // once it has moved by 5 % it reads as before
        let moved = sized(Unit::Volt, "gpu0/power/voltage", 0.87, 0.80, 0.90);
        let g = geometry_in(&moved, &Thr::default(), None).unwrap();
        assert!(g.range.is_some() && g.marker.is_some());
        // none of this touches percentages, temperatures, capacities or readings with thresholds
        let pct = sized(Unit::Percent, "cpu/usage/total", 5.0, 5.0, 5.0);
        assert!(geometry_in(&pct, &Thr::default(), None).unwrap().marker.is_some());
        let temp = sized(Unit::Celsius, "cpu/temp/tctl", 50.0, 50.0, 50.0);
        assert!(geometry_in(&temp, &Thr::default(), None).unwrap().marker.is_some());
        let mem = sized(Unit::Bytes, "memory/usage/used", 20.0 * GIB, 20.0 * GIB, 20.0 * GIB);
        assert!(geometry_in(&mem, &Thr::default(), Some(64.0 * GIB)).unwrap().marker.is_some());
        let fan = sized(Unit::Rpm, "board/fan/x", 1000.0, 1000.0, 1000.0);
        let thr = Thr { warn: Some(500.0), crit: Some(300.0), below: true };
        assert!(geometry_in(&fan, &thr, None).unwrap().marker.is_some());
    }

    #[test]
    fn a_range_that_covers_most_of_the_track_is_drawn_lighter() {
        // disk read: 0 to the burst peak on a scale that ends 5 % past the peak
        let disk = sized(Unit::BytesPerSec, "disk/x/io/read", 1.0e6, 0.0, 2.95e9);
        let g = geometry_in(&disk, &Thr::default(), None).unwrap();
        assert!(g.quiet, "{:?}", g.range);
        // a narrow range keeps the full fill
        let temp = sized(Unit::Celsius, "cpu/temp/tctl", 55.0, 45.0, 60.0);
        assert!(!geometry_in(&temp, &Thr::default(), None).unwrap().quiet);
        // exactly the coverage threshold is quiet, just under it is not
        let edge = |hi: f64| {
            let r = sized(Unit::Percent, "x", 10.0, 0.0, hi);
            geometry_in(&r, &Thr::default(), None).unwrap().quiet
        };
        assert!(edge(60.0) && edge(95.0) && !edge(59.0));
        // the lighter fill keeps the marker and the average tick readable (3:1 on the fill they sit on)
        for pal in [Palette::dark(), Palette::light()] {
            for device in [pal.cpu, pal.gpu, pal.mem, pal.drive, pal.board, pal.igpu] {
                let track = track_color(device, &pal);
                let quiet = track.blend(device.gamma_multiply(pal.range_alpha * QUIET_FILL));
                let full = track.blend(device.gamma_multiply(pal.range_alpha));
                // lighter than the full fill: closer to the track than the full fill is
                let dist = |a: Color32, b: Color32| (a.r() as i32 - b.r() as i32).abs() + (a.g() as i32 - b.g() as i32).abs() + (a.b() as i32 - b.b() as i32).abs();
                assert!(dist(quiet, track) * 3 < dist(full, track) * 2, "{device:?}");
                // the marker and the average tick sit on the fill: they read at least as well on the
                // lighter one as on the full one
                for mark in [pal.ink, pal.ink3] {
                    assert!(crate::ui_kit::contrast(mark, quiet) >= crate::ui_kit::contrast(mark, full), "{device:?}");
                }
                assert!(crate::ui_kit::contrast(pal.ink, quiet) >= 3.0, "the marker on the quiet fill");
            }
        }
    }

    #[test]
    fn fractions_are_clamped() {
        // an observed value outside a fixed domain stays on the track
        let r = reading(Unit::Percent, 120.0, 10.0, 120.0, 50.0);
        let g = geometry(&r, &Thr::default()).unwrap();
        assert_eq!(g.marker, Some(1.0));
    }
}
