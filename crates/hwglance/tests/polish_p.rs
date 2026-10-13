//! Stage 7 polish pass: the key screens rendered off-screen in the palette named by
//! `HWGLANCE_THEME`, into `HWGLANCE_SHOTS` (file names end in the theme). Without
//! `HWGLANCE_SHOTS` the tests only check that the scenes build.

mod common;

use common::*;
use egui::pos2;
use egui_kittest::kittest::Queryable;
use hwglance::app::{KillState, KillWatch, SettingsTab};
use hwglance::settings::{Dock, Settings};
use hwglance_proto::{Crossing, CrossingLevel, FlagEvents, Snapshot, Stats, Unit, PROTO_VERSION};
use std::time::Instant;

const REAL: (f32, f32) = (2546.0, 1400.0);
const MIN: u64 = 60_000;

fn theme() -> String {
    std::env::var("HWGLANCE_THEME").unwrap_or_else(|_| "dark".into())
}

fn shot(h: &mut H, name: &str) {
    save(h, &format!("{name}-{}.png", theme()));
}

fn park(h: &mut H) {
    h.hover_at(pos2(1500.0, 3.0));
    h.run_steps(3);
}

fn readings(s: &mut Snapshot) -> impl Iterator<Item = &mut hwglance_proto::Reading> {
    s.sensors.iter_mut().flat_map(|g| g.sections.iter_mut()).flat_map(|x| x.readings.iter_mut())
}

/// The real fixture after a few hours: one warm CPU, a throttle and three crossings.
fn lived_in() -> Snapshot {
    let mut s = repo_fixture();
    s.proto = PROTO_VERSION;
    s.history_minutes = 30;
    let now = s.unix_ms;
    s.stats_since_unix_ms = now - 4 * 3_600_000;
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
            _ => continue,
        };
        if r.id == "memory/speed/clock" || r.id == "gpu0/power/limit" {
            continue;
        }
        r.stats = Some(Stats { min: lo, max: hi, avg, samples: 600 });
    }
    let tctl = readings(&mut s).find(|r| r.id == "cpu/temp/tctl").unwrap();
    tctl.value = Some(83.7);
    tctl.stats = Some(Stats { min: 48.0, max: 84.2, avg: 62.0, samples: 600 });
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

#[test]
fn p01_default_bottom() {
    let mut h = procs_harness("p-default", REAL, Settings::default());
    shot(&mut h, "p01-default-bottom");
}

#[test]
fn p02_default_1200x700() {
    let (snap, _) = demo_scene(hwglance::demo::T_START + 30.0);
    let mut h = harness("p-small", (1200.0, 700.0), snap, Some(Settings::default()), true, false);
    park(&mut h);
    shot(&mut h, "p02-default-1200x700");
}

#[test]
fn p03_right_compact() {
    let mut s = Settings::default();
    s.layout.set_dock(Dock::Right);
    let mut h = procs_harness("p-rc", REAL, s);
    shot(&mut h, "p03-right-compact");
}

#[test]
fn p04_right_expanded() {
    let mut s = Settings::default();
    s.layout.set_dock(Dock::Right);
    let mut h = procs_harness("p-re", REAL, expanded_view(s));
    shot(&mut h, "p04-right-expanded");
}

#[test]
fn p05_history_popover() {
    let mut h = popover_harness("p-pop", REAL, None, "gpu0", "gpu0/power/board");
    shot(&mut h, "p05-history-popover");
}

#[test]
fn p06_crossings_list() {
    let mut h = harness("p-cross", REAL, lived_in(), Some(Settings::default()), true, false);
    park(&mut h);
        h.query_all_by_label_contains("3 crossings since").next().expect("the chip").click();
    h.run_steps(4);
    park(&mut h);
    shot(&mut h, "p06-crossings-list");
}

#[test]
fn p07_process_panel_end_task_outcome() {
    let mut h = procs_harness("p-kill", REAL, expanded_view(Settings::default()));
    let a = app(&mut h);
    let t = a.selected_target().expect("a selection");
    a.kill_watch = Some(KillWatch {
        row: a.selection.clone().unwrap(),
        name: t.name.clone(),
        signal: hwglance::actions::Signal::Term,
        procs: t.procs.clone(),
        state: KillState::StillRunning,
        since: Instant::now(),
        message: None,
    });
    a.kill_outcome_for = std::time::Duration::from_secs(600);
    h.run_steps(4);
    shot(&mut h, "p07-end-task-still-running");
    let a = app(&mut h);
    a.kill_watch.as_mut().unwrap().state = KillState::Ended;
    h.run_steps(4);
    shot(&mut h, "p07b-end-task-ended");
}

#[test]
fn p08_value_menu_thresholds() {
    let (snap, _) = demo_scene(hwglance::demo::T_START + 30.0);
    let mut h = harness("p-menu", REAL, snap, Some(Settings::default()), true, false);
    park(&mut h);
    let cpu = app(&mut h).card_rects.iter().find(|(c, _)| c == "cpu").unwrap().1;
    click_secondary(&mut h, pos2(cpu.left() + 100.0, cpu.top() + 36.0 + 20.0 + 28.0 + 14.0));
    shot(&mut h, "p08-value-menu");
}

#[test]
fn p09_settings_tabs() {
    let mut h = harness("p-settings", REAL, repo_fixture(), Some(Settings::default()), true, true);
    for (tab, name) in [
        (SettingsTab::General, "p09a-settings-general"),
        (SettingsTab::Sensors, "p09b-settings-sensors"),
        (SettingsTab::Alerts, "p09c-settings-alerts"),
        (SettingsTab::BarWidget, "p09d-settings-bar-widget"),
    ] {
        app(&mut h).settings_tab = tab;
        h.run_steps(4);
        shot(&mut h, name);
    }
}

#[test]
fn p10_edit_layout() {
    let (snap, _) = demo_scene(hwglance::demo::T_START + 30.0);
    let mut h = harness("p-edit", REAL, snap, Some(Settings::default()), true, false);
    app(&mut h).edit_layout = true;
    h.run_steps(3);
    park(&mut h);
    shot(&mut h, "p10-edit-layout");
}

#[test]
fn p11_collector_down() {
    let mut h = connecting_harness((1400.0, 700.0), None);
    shot(&mut h, "p11-collector-down");
}

#[test]
fn p12_force_kill_dialog() {
    let mut h = procs_harness("p-dialog", REAL, Settings::default());
    app(&mut h).request_kill(None, hwglance::actions::Signal::Kill);
    h.run_steps(4);
    shot(&mut h, "p12-force-kill-dialog");
}

#[test]
fn the_threshold_fields_of_the_value_menu_share_a_left_edge() {
    let _g = layout_lock();
    let (snap, _) = demo_scene(hwglance::demo::T_START + 30.0);
    let mut h = harness("p-menu-align", REAL, snap, Some(Settings::default()), false, false);
    park(&mut h);
    let cpu = app(&mut h).card_rects.iter().find(|(c, _)| c == "cpu").unwrap().1;
    click_secondary(&mut h, pos2(cpu.left() + 100.0, cpu.top() + 36.0 + 20.0 + 28.0 + 14.0));
    let lefts: Vec<f32> = h.query_all_by_role(egui::accesskit::Role::SpinButton).map(|n| n.rect().left()).collect();
    assert_eq!(lefts.len(), 2, "warning and critical fields: {lefts:?}");
    assert!((lefts[0] - lefts[1]).abs() < 0.5, "the fields start at {lefts:?}");
}
