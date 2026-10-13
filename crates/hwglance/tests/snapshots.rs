//! Golden-image tests: curated states of the real UI rendered offscreen (wgpu) and compared with
//! the PNGs in `tests/snapshots/`. Refresh them after an intended visual change with
//! `UPDATE_SNAPSHOTS=1 cargo test -p hwglance --test snapshots` and review the diff.
//!
//! Every input is fixed so a diff means the UI changed: the repo fixture or the demo on a fixed
//! clock, the render size, the embedded fonts, the theme (named per test, never "system"), the
//! "updated .. s ago" age, the time zone (UTC) and the icon search path (none).

mod common;

use common::*;
use egui_kittest::{SnapshotOptions, SnapshotResults};
use hwglance::settings::{Dock, Settings};
use std::sync::Once;

/// Render size of every golden but the small window: keeps the PNGs near 100 KB each.
const SIZE: (f32, f32) = (1600.0, 900.0);
const SMALL: (f32, f32) = (1200.0, 700.0);
const CONNECTING: (f32, f32) = (1400.0, 600.0);
const BOTH: [&str; 2] = ["dark", "light"];

/// How far one pixel may differ (weighted YIQ distance, see `SnapshotOptions::threshold`) and how
/// many pixels may exceed that. A changed colour token or a moved border moves far more than a
/// few anti-aliased edge pixels; GPU rounding noise stays below both.
const THRESHOLD: f32 = 0.2;
const MAX_FAILED_PIXELS: usize = 16;

unsafe extern "C" {
    fn tzset();
}

/// Pin the process-wide inputs that would otherwise follow the machine: the time zone of the
/// clock texts and the icon themes of the process rows.
fn setup() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        // SAFETY: runs once before any test of this binary builds a harness or reads these.
        unsafe {
            std::env::set_var("TZ", "UTC");
            std::env::set_var("HWGLANCE_ICON_DIRS", "/nonexistent-hwglance-icons");
            tzset();
        }
    });
}

fn themed(theme: &str, mut s: Settings) -> Settings {
    s.appearance.theme = theme.into();
    s
}

/// Pin the age shown as "updated 0.6 s ago" (the real one is a wall-clock reading).
fn fixed(mut h: H) -> H {
    app(&mut h).link.shared.lock().unwrap().pinned_age = Some(0.6);
    h.run_steps(3);
    h
}

/// Build `name` in each of `themes` and compare the render with `tests/snapshots/{name}_{theme}.png`.
fn check(name: &str, themes: &[&str], build: impl Fn(&str) -> H) {
    // The goldens come from a real GPU; a CI runner without one sets this to skip the comparison.
    if std::env::var_os("HWGLANCE_SKIP_GOLDEN").is_some_and(|v| v == "1") {
        return;
    }
    setup();
    let _g = layout_lock();
    let options = SnapshotOptions::new().threshold(THRESHOLD).max_failed_pixels(MAX_FAILED_PIXELS);
    let mut results = SnapshotResults::new();
    for theme in themes {
        let mut h = fixed(build(theme));
        h.snapshot_options(format!("{name}_{theme}"), &options);
        results.extend_harness(&mut h);
    }
    results.unwrap();
}

#[test]
fn default_bottom_dock() {
    check("default", &BOTH, |t| harness("snap-default", SIZE, repo_fixture(), Some(themed(t, Settings::default())), true, false));
}

#[test]
fn watched_values() {
    check("watched", &BOTH, |t| harness("snap-watched", SIZE, watched_snapshot(repo_fixture()), Some(themed(t, watched_settings())), true, false));
}

#[test]
fn history_popover() {
    check("history-popover", &BOTH, |t| popover_harness("snap-popover", SIZE, Some(themed(t, Settings::default())), "cpu", "cpu/power/cores"));
}

#[test]
fn processes_expanded() {
    check("processes-expanded", &BOTH, |t| procs_harness("snap-procs", SIZE, themed(t, expanded_view(Settings::default()))));
}

#[test]
fn dock_right_compact() {
    check("dock-right-compact", &BOTH, |t| {
        let mut s = themed(t, Settings::default());
        s.layout.set_dock(Dock::Right);
        procs_harness("snap-right-compact", SIZE, s)
    });
}

#[test]
fn dock_right_expanded_stacked_columns() {
    // Four explicit columns of 496 px that stack into fewer display columns beside the panel.
    check("dock-right-expanded", &BOTH, |t| {
        let s = themed(t, users_arrangement(Dock::Right, "expanded", Some(0.474)));
        arrangement_harness("snap-right-expanded", SIZE, s, false)
    });
}

#[test]
fn settings_general() {
    check("settings-general", &BOTH, |t| {
        let mut h = harness("snap-general", SIZE, repo_fixture(), Some(themed(t, Settings::default())), true, true);
        app(&mut h).settings_tab = hwglance::app::SettingsTab::General;
        h.run_steps(4);
        h
    });
}

#[test]
fn edit_layout() {
    check("edit-layout", &BOTH, |t| {
        let mut h = harness("snap-edit", SIZE, repo_fixture(), Some(themed(t, Settings::default())), true, false);
        app(&mut h).edit_layout = true;
        h.run_steps(3);
        h
    });
}

#[test]
fn collector_not_running() {
    check("connecting", &["dark"], |t| connecting_harness(CONNECTING, Some(themed(t, Settings::default()))));
}

#[test]
fn small_window() {
    check("small-1200x700", &["light"], |t| harness("snap-small", SMALL, repo_fixture(), Some(themed(t, Settings::default())), true, false));
}
