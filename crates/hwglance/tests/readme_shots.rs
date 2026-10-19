//! README screenshots, rendered offscreen from the demo data (no real machine data involved).
//! Ignored by default; `scripts/readme-shots.sh` runs it and writes `docs/screenshots/`.
//! Set `HWGLANCE_README_SHOTS=<dir>` to write somewhere else.

mod common;

use common::*;
use hwglance::app::SettingsTab;
use hwglance::settings::{Dock, LogPrefs, Settings};
use hwglance_proto::LogStatus;
use std::path::{Path, PathBuf};

const SIZE: (f32, f32) = (1600.0, 900.0);
/// The settings panel and a margin of the window around it (x, y, width, height), for the
/// Logging shot: the panel is a fixed size, so the full 1600 x 900 render would show it small.
const SETTINGS_CROP: (u32, u32, u32, u32) = (620, 36, 900, 654);

unsafe extern "C" {
    fn tzset();
}

fn out_dir() -> PathBuf {
    std::env::var_os("HWGLANCE_README_SHOTS")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/screenshots"))
}

/// Pin the inputs that would follow the machine: the time zone and the icon themes.
fn setup() {
    // SAFETY: runs before any harness of this test is built.
    unsafe {
        std::env::set_var("TZ", "UTC");
        std::env::set_var("HWGLANCE_ICON_DIRS", concat!(env!("CARGO_MANIFEST_DIR"), "/tests/assets/icons"));
        tzset();
    }
}

fn themed(theme: &str, mut s: Settings) -> Settings {
    s.appearance.theme = theme.into();
    s
}

/// Pin the "updated .. s ago" age (the real one is a wall-clock reading), then render.
fn render_pinned(h: &mut H) -> image::RgbaImage {
    app(h).link.shared.lock().unwrap().pinned_age = Some(0.6);
    h.run_steps(3);
    h.render().expect("render")
}

fn write_png(img: &image::RgbaImage, name: &str) {
    let dir = out_dir();
    std::fs::create_dir_all(&dir).unwrap();
    img.save(dir.join(name)).unwrap();
}

fn shot(mut h: H, name: &str) {
    write_png(&render_pinned(&mut h), name);
}

#[test]
#[ignore]
fn readme_screenshots() {
    setup();
    let _g = layout_lock();
    let at = hwglance::demo::T_START + 140.0;

    for theme in ["dark", "light"] {
        let (snap, _) = demo_scene(at);
        let h = harness("readme-main", SIZE, snap, Some(themed(theme, Settings::default())), true, false);
        shot(h, &format!("main-{theme}.png"));
    }

    let mut s = themed("dark", Settings::default());
    s.layout.set_dock(Dock::Right);
    let mut h = procs_harness("readme-dock", SIZE, s);
    // No pointer arrow in the render.
    h.event(egui::Event::PointerGone);
    h.run_steps(3);
    shot(h, "dock-right-compact.png");
}

/// Settings, Logging with four values ticked and the collector writing. The window reads the
/// log folder (its placeholder and the "holds N files" line) from `$HOME`, so point that at a
/// neutral user for the render and put the machine's back afterwards.
#[test]
#[ignore]
fn readme_logging_screenshot() {
    setup();
    let _g = layout_lock();
    let saved = (std::env::var_os("HOME"), std::env::var_os("XDG_STATE_HOME"));
    // SAFETY: the layout lock is held, so no other harness of this test binary is running.
    unsafe {
        std::env::set_var("HOME", "/home/alex");
        std::env::remove_var("XDG_STATE_HOME");
    }
    let (mut snap, _) = demo_scene(hwglance::demo::T_START + 140.0);
    snap.logging = Some(LogStatus {
        file: Some("/home/alex/.local/state/hwglance/logs/hwglance-2026-10-19_082259.csv".into()),
        columns: 4,
        interval_s: 1,
        error: None,
    });
    let mut s = themed("dark", Settings::default());
    s.logging = LogPrefs {
        values: ["cpu/temp/tctl", "gpu0/temp/hotspot", "gpu0/load/core", "cpu/power/package"].map(String::from).to_vec(),
        ..LogPrefs::default()
    };
    let mut h = harness("readme-logging", SIZE, snap, Some(s), true, true);
    app(&mut h).settings_tab = SettingsTab::Logging;
    h.run_steps(4);
    let full = render_pinned(&mut h);
    let (x, y, w, hgt) = SETTINGS_CROP;
    write_png(&image::imageops::crop_imm(&full, x, y, w, hgt).to_image(), "settings-logging.png");
    // SAFETY: as above.
    unsafe {
        match saved.0 {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
        match saved.1 {
            Some(v) => std::env::set_var("XDG_STATE_HOME", v),
            None => std::env::remove_var("XDG_STATE_HOME"),
        }
    }
}
