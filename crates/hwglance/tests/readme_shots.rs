//! README screenshots, rendered offscreen from the demo data (no real machine data involved).
//! Ignored by default; `scripts/readme-shots.sh` runs it and writes `docs/screenshots/`.
//! Set `HWGLANCE_README_SHOTS=<dir>` to write somewhere else.

mod common;

use common::*;
use hwglance::settings::{Dock, Settings};
use std::path::{Path, PathBuf};

const SIZE: (f32, f32) = (1600.0, 900.0);

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

/// Pin the "updated .. s ago" age (the real one is a wall-clock reading), then write the render.
fn shot(mut h: H, name: &str) {
    app(&mut h).link.shared.lock().unwrap().pinned_age = Some(0.6);
    h.run_steps(3);
    let dir = out_dir();
    std::fs::create_dir_all(&dir).unwrap();
    h.render().expect("render").save(dir.join(name)).unwrap();
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
