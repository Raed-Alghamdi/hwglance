//! Batch B in the window: crossings after an alert (the top-bar chip and its list, the legend
//! rings, the popover's onset and extremes), range bars that read as what they are, a quieter
//! glance, throttling that stops being amber, stale data that keeps its contrast, and sources
//! that fail where you can see them. Writes PNGs into `HWGLANCE_SHOTS` when it is set (a `light`
//! variant under `HWGLANCE_THEME=light`).

mod common;

use common::*;
use egui::pos2;
use egui_kittest::kittest::Queryable;
use hwglance::settings::Settings;
use hwglance_proto::{Crossing, CrossingLevel, FlagEvents, HistorySeries, Snapshot, Stats, Unit, PROTO_VERSION};
use std::time::{Duration, Instant};

const MIN: u64 = 60_000;
const SIZE: (f32, f32) = (2546.0, 1400.0);
const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

/// Is some node's label containing `text` on screen (several nodes may carry the same words)?
fn has(h: &H, text: &str) -> bool {
    h.query_all_by_label_contains(text).next().is_some()
}

fn theme() -> String {
    std::env::var("HWGLANCE_THEME").unwrap_or_else(|_| "dark".into())
}

fn readings(s: &mut Snapshot) -> impl Iterator<Item = &mut hwglance_proto::Reading> {
    s.sensors.iter_mut().flat_map(|g| g.sections.iter_mut()).flat_map(|x| x.readings.iter_mut())
}

fn set_stats(s: &mut Snapshot, id: &str, min: f64, max: f64, avg: f64) {
    let r = readings(s).find(|r| r.id == id).unwrap_or_else(|| panic!("{id}"));
    r.stats = Some(Stats { min, max, avg, samples: 600 });
}

/// The real fixture after a few hours of use: ranges that have spread, one warm CPU, a throttle
/// from earlier and the collector's record of three crossings (one still going).
fn lived_in() -> Snapshot {
    let mut s = repo_fixture();
    s.proto = PROTO_VERSION;
    s.history_minutes = 30;
    let now = s.unix_ms;
    s.stats_since_unix_ms = now - 4 * 3_600_000;
    s.summary.swap_total_bytes = Some(8.0 * GIB);
    for r in readings(&mut s) {
        let Some(v) = r.value else { continue };
        let (lo, hi, avg) = match r.unit {
            Unit::Percent => (0.5, (v * 3.0 + 20.0).min(100.0), v * 1.4),
            Unit::Celsius => (v - 12.0, v + 14.0, v),
            Unit::Watt => (v * 0.4, v * 2.2, v * 0.9),
            Unit::Bytes => (v * 0.8, v * 1.1, v),
            Unit::BytesPerSec => (0.0, 2.95e9, 1.0e7),
            Unit::Mhz => (v * 0.7, v * 1.2, v),
            Unit::Rpm => (v * 0.6, v * 1.2, v),
            _ => continue, // volts stay flat, flags and the configured memory speed have no range
        };
        if r.id == "memory/speed/clock" || r.id == "gpu0/power/limit" {
            continue;
        }
        r.stats = Some(Stats { min: lo, max: hi, avg, samples: 600 });
    }
    // The CPU is warm right now: past its warning level and flagged H.
    let tctl = readings(&mut s).find(|r| r.id == "cpu/temp/tctl").unwrap();
    tctl.value = Some(83.7);
    set_stats(&mut s, "cpu/temp/tctl", 48.0, 84.2, 62.0);
    // CPU throttled 3 minutes ago (amber), GPU 40 minutes ago (quiet).
    for (id, last, ended) in [("cpu/throttle/thermal", now - 3 * MIN, now - 2 * MIN), ("gpu0/throttle/thermal", now - 40 * MIN, now - 38 * MIN)] {
        let r = readings(&mut s).find(|r| r.id == id).unwrap();
        r.events = Some(FlagEvents { count: 3, last_unix_ms: last, ended_unix_ms: Some(ended) });
        r.stats = Some(Stats { min: 0.0, max: 1.0, avg: 0.02, samples: 600 });
    }
    let c = |id: &str, level, start: u64, peak: f64, end: Option<u64>| Crossing { id: id.into(), level, start_ms: start, peak, peak_ms: start + 2 * MIN, end_ms: end, below: false };
    s.crossings = vec![
        c("gpu0/temp/hotspot", CrossingLevel::Crit, now - 52 * MIN, 106.0, Some(now - 47 * MIN)),
        c("igpu/main/temp", CrossingLevel::Warn, now - 12 * MIN, 79.0, Some(now - 8 * MIN)),
        c("cpu/temp/tctl", CrossingLevel::Warn, now - 6 * MIN, 84.2, None),
    ];
    s
}

fn settings() -> Settings {
    let mut s = Settings::default();
    s.appearance.theme = theme();
    s
}

fn scene(name: &str) -> H {
    let mut h = harness(name, SIZE, lived_in(), Some(settings()), true, false);
    h.hover_at(pos2(1500.0, 3.0));
    h.run_steps(3);
    h
}

#[test]
fn the_top_bar_counts_the_crossings_and_its_list_leads_to_the_reading() {
    let _guard = layout_lock();
    let mut h = scene("b-crossings");
    save(&mut h, &format!("b1-crossings-chip-{}.png", theme()));
    h.query_all_by_label_contains("3 crossings since").next().expect("the chip").click();
    h.run_steps(4);
    // newest first, each with the reading, how bad it got and when
    assert!(has(&h, "Threshold crossings"), "the list is open");
    assert!(has(&h, "CPU, Tctl has been above its warning level since"));
    assert!(has(&h, "GPU, Hotspot was above its critical level from"));
    assert!(has(&h, "iGPU, Edge temperature was above its warning level from"));
    h.hover_at(pos2(1500.0, 3.0));
    h.run_steps(2);
    save(&mut h, &format!("b2-crossings-list-{}.png", theme()));
    // a row scrolls to the reading and pins its history
    h.query_all_by_label_contains("GPU, Hotspot was above").next().expect("the row").click();
    h.run_steps(5);
    assert_eq!(app(&mut h).pinned.as_deref(), Some("gpu0/temp/hotspot"));
    assert!(app(&mut h).flash.as_ref().is_some_and(|f| f.0 == "gpu0/temp/hotspot"), "and flashes it");
    assert!(!has(&h, "Threshold crossings"), "the list closes");
}

#[test]
fn a_collector_without_crossings_shows_no_chip() {
    let mut s = lived_in();
    s.crossings.clear();
    let h = harness("b-no-crossings", SIZE, s, Some(settings()), false, false);
    assert!(!has(&h, "crossing"));
}

#[test]
fn crossings_survive_a_reconnect_because_the_next_snapshot_brings_them() {
    // the window keeps nothing: a snapshot published after a reconnect carries the whole list
    let mut h = harness("b-reconnect", SIZE, lived_in(), Some(settings()), false, false);
    assert!(has(&h, "3 crossings since"));
    let ctx = h.ctx.clone();
    let mut next = lived_in();
    next.seq += 1;
    next.crossings.retain(|c| c.id != "igpu/main/temp");
    app(&mut h).link.publish(&ctx, next);
    h.run_steps(4);
    assert!(has(&h, "2 crossings since"), "the chip follows the collector's list");
}

#[test]
fn the_igpu_has_a_legend_chip_and_recent_crossings_show_on_the_chips() {
    let h = harness("b-legend", SIZE, lived_in(), Some(settings()), false, false);
    assert!(has(&h, "Jump to the iGPU panel"), "the iGPU has a chip");
    // the chip tips name the crossing, in words
    assert!(has(&h, "iGPU, Edge temperature was above its warning level from"));
    assert!(has(&h, "CPU: Tctl is above its warning level"));
    // a device without a recent crossing keeps its plain tip
    assert!(has(&h, "Jump to the Memory panel"));
    // the GPU's hotspot crossing is 47 minutes old: outside the 30 minute window, no ring and no sentence on its chip
    assert!(has(&h, "Jump to the GPU panel"));
    // no iGPU card, no chip
    let mut s = lived_in();
    s.sensors.retain(|g| g.id != "igpu");
    s.crossings.retain(|c| c.id != "igpu/main/temp");
    let h = harness("b-legend2", SIZE, s, Some(settings()), false, false);
    assert!(!has(&h, "iGPU panel"));
}

#[test]
fn the_popover_always_names_the_onset_and_labels_its_extremes() {
    let _guard = layout_lock();
    let mut h = scene("b-popover");
    let snap = lived_in();
    let now = snap.unix_ms;
    let ctx = h.ctx.clone();
    let points: Vec<(u64, f32)> = (0..360)
        .map(|i| {
            let t = now - 1_800_000 + i as u64 * 5000;
            let hump = if (250..340).contains(&i) { 22.0 * (1.0 - ((i as f32 - 295.0) / 45.0).powi(2)).max(0.0) } else { 0.0 };
            (t, 58.0 + hump + ((i * 7) % 5) as f32 * 0.4)
        })
        .collect();
    app(&mut h).link.publish_history(&ctx, vec![HistorySeries { id: "cpu/temp/tctl".into(), unit: Unit::Celsius, points }]);
    app(&mut h).pinned = Some("cpu/temp/tctl".into());
    h.hover_at(pos2(5.0, 5.0));
    h.run_steps(5);
    // the onset is the collector's: going since 6 minutes ago, with the peak
    assert!(has(&h, "Above warning since"), "the onset note");
    assert!(has(&h, "peak 84.2"));
    save(&mut h, &format!("b3-popover-onset-{}.png", theme()));
    // a crossing that is over: both times
    app(&mut h).pinned = Some("gpu0/temp/hotspot".into());
    app(&mut h).pinned_hit = None;
    h.run_steps(5);
    app(&mut h).scroll_to_reading = Some("gpu0/temp/hotspot".into());
    h.run_steps(5);
    assert!(has(&h, "Last crossed critical"), "the most recent crossing, after it cooled");
}

#[test]
fn past_throttling_is_amber_for_ten_minutes_and_quiet_after() {
    // the cards: CPU throttled 3 minutes ago, GPU 40 minutes ago; the row notes follow the same clock
    let mut h = scene("b-throttle");
    save(&mut h, &format!("b4-throttle-decay-{}.png", theme()));
    let snap = lived_in();
    let cpu = snap.sensors.iter().find(|g| g.id == "cpu").unwrap();
    let gpu = snap.sensors.iter().find(|g| g.id == "gpu0").unwrap();
    use hwglance::limits::{find_thermal, throttle_state, Throttle};
    assert!(matches!(throttle_state(find_thermal(cpu).unwrap(), snap.unix_ms), Throttle::Past { recent: true, .. }));
    assert!(matches!(throttle_state(find_thermal(gpu).unwrap(), snap.unix_ms), Throttle::Past { recent: false, .. }));
    // ten minutes after its end the CPU's goes quiet too (the window follows the snapshot clock)
    assert!(matches!(throttle_state(find_thermal(cpu).unwrap(), snap.unix_ms + 7 * MIN), Throttle::Past { recent: true, .. }));
    assert!(matches!(throttle_state(find_thermal(cpu).unwrap(), snap.unix_ms + 9 * MIN), Throttle::Past { recent: false, .. }));
    let _ = &mut h;
}

#[test]
fn range_bars_use_what_is_known_and_stay_quiet() {
    let mut h = scene("b-bars");
    save(&mut h, &format!("b5-range-bars-{}.png", theme()));
    let _ = &mut h;
}

#[test]
fn stale_data_keeps_its_contrast_and_says_since_when() {
    let _guard = layout_lock();
    let fresh = {
        let mut h = harness("b-fresh", SIZE, lived_in(), Some(settings()), true, false);
        h.hover_at(pos2(1500.0, 3.0));
        h.run_steps(3);
        h.render().expect("render")
    };
    let mut h = harness("b-stale", SIZE, lived_in(), Some(settings()), true, false);
    {
        let a = app(&mut h);
        let mut s = a.link.shared.lock().unwrap();
        s.never_stale = false;
        s.pinned_age = None;
        s.received = Some(Instant::now() - Duration::from_secs(47));
    }
    h.hover_at(pos2(1500.0, 3.0));
    h.run_steps(4);
    save(&mut h, &format!("b6-stale-{}.png", theme()));
    let stale = h.render().expect("render");
    // the stamp: the time of the snapshot and how long ago
    assert!(has(&h, "as of "), "the status bar says as of when");
    assert!(has(&h, "s ago"));
    // the sensor area is pixel for pixel what it is when fresh: nothing is dimmed
    let (w, hgt) = (fresh.width(), fresh.height());
    assert_eq!((w, hgt), (stale.width(), stale.height()));
    let mut differing = 0usize;
    for y in 90..(hgt - 60) {
        for x in 0..(w * 6 / 10) {
            if fresh.get_pixel(x, y) != stale.get_pixel(x, y) {
                differing += 1;
            }
        }
    }
    assert_eq!(differing, 0, "{differing} pixels of the cards differ between fresh and stale");
}

#[test]
fn a_failed_source_says_so_inside_its_card() {
    let mut s = lived_in();
    // NVML fails: the collector sends no GPU card at all
    s.sensors.retain(|g| g.id != "gpu0");
    s.crossings.retain(|c| !c.id.starts_with("gpu0"));
    for src in s.sources.iter_mut().filter(|x| x.name == "nvml") {
        src.ok = false;
        src.detail = "libnvidia-ml.so.1 not found".into();
    }
    let mut h = harness("b-nvml", SIZE, s, Some(settings()), true, false);
    assert!(has(&h, "GPU readings unavailable: libnvidia-ml.so.1 not found"));
    save(&mut h, &format!("b7-source-nvml-{}.png", theme()));
    // LACT fails: the GPU card stays and carries the collector's own sentence
    let mut s = lived_in();
    for src in s.sources.iter_mut().filter(|x| x.name == "lact") {
        src.ok = false;
        src.detail = "LACT unavailable: hotspot, VRAM temperatures, fan RPM and core voltage missing (connection refused)".into();
    }
    let h = harness("b-lact", SIZE, s, Some(settings()), false, false);
    assert!(has(&h, "LACT unavailable: hotspot"));
    // every source fine: no note
    let h = harness("b-fine", SIZE, lived_in(), Some(settings()), false, false);
    assert!(!has(&h, "unavailable"));
}

#[test]
fn a_failed_start_shows_its_error_on_the_collector_down_screen() {
    let _guard = layout_lock();
    let mut h = connecting_harness((1400.0, 800.0), Some(settings()));
    assert!(has(&h, "Collector not running"));
    assert!(!has(&h, "Could not start the collector"));
    {
        let a = app(&mut h);
        let mut s = a.link.shared.lock().unwrap();
        s.action_error = Some("Could not start the collector: Failed to start hwglanced.service: Unit hwglanced.service not found.".into());
        s.action_failures += 1;
    }
    h.run_steps(4);
    assert!(has(&h, "Unit hwglanced.service not found"));
    save(&mut h, &format!("b8-start-failed-{}.png", theme()));
}
