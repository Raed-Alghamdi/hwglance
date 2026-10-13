#![allow(dead_code)]
//! Shared helpers for the offscreen UI tests. Every test gets its own ui.json in a temp
//! directory, so the real `~/.config/hwglance/ui.json` is never touched.

use egui_kittest::Harness;
use hwglance::app::{App, Mode, Opts};
use hwglance::demo::Demo;
use hwglance::settings::Settings;
use hwglance_proto::Snapshot;
use std::path::PathBuf;

pub type H = Harness<'static, Option<App>>;

static LAYOUT_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Tests that click at pixel positions, or that change the global font size, hold this so the
/// font-size test cannot move rows under another test's pointer.
pub fn layout_lock() -> std::sync::MutexGuard<'static, ()> {
    LAYOUT_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("hwglance-tests-{}", std::process::id())).join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

pub fn demo_snapshot() -> Snapshot {
    Demo::new().snapshot(12.0)
}

pub fn fixture_snapshot() -> Snapshot {
    if let Some(p) = std::env::var_os("HWGLANCE_FIXTURE") {
        let text = std::fs::read_to_string(p).unwrap();
        return hwglance::conn::parse_mock(&text).unwrap();
    }
    demo_snapshot()
}

pub fn shots_dir() -> Option<PathBuf> {
    std::env::var_os("HWGLANCE_SHOTS").map(PathBuf::from)
}

/// `HWGLANCE_THEME=light|dark|system` sets `appearance.theme` of every harness, so the whole
/// suite (and the shots) can run in the other palette. Unset leaves the settings alone, and
/// settings that already name "dark" or "light" keep it (the snapshot tests pin their theme).
pub fn with_theme(settings: Option<Settings>) -> Option<Settings> {
    let Ok(theme) = std::env::var("HWGLANCE_THEME") else { return settings };
    let mut s = settings.unwrap_or_default();
    if s.appearance.theme == "system" {
        s.appearance.theme = theme;
    }
    Some(s)
}

/// Start the app on a mock snapshot with the given initial settings. `render` adds the wgpu
/// renderer so `save` can write PNGs.
pub fn harness(name: &str, size: (f32, f32), snap: Snapshot, settings: Option<Settings>, render: bool, open_settings: bool) -> H {
    let dir = tmp(name);
    let mock = dir.join("snapshot.json");
    std::fs::write(&mock, serde_json::to_vec(&snap).unwrap()).unwrap();
    let ui_json = dir.join("ui.json");
    if let Some(s) = with_theme(settings) {
        hwglance::settings::save_atomic(&ui_json, &s, false).unwrap();
    }
    let opts = Opts { settings_path: Some(ui_json), open_settings, no_watchers: true, ..Default::default() };
    let b = Harness::builder().with_size(egui::vec2(size.0, size.1));
    let b = if render { b.wgpu() } else { b };
    let mut h = b.build_ui_state(
        move |ui, app: &mut Option<App>| {
            // Fonts set at creation only apply from the next pass, so skip drawing once.
            if app.is_none() {
                *app = Some(App::with_ctx(ui.ctx(), None, Mode::Mock(mock.clone()), opts.clone()));
                return;
            }
            app.as_mut().unwrap().show(ui);
        },
        None,
    );
    h.run_steps(6);
    h
}

/// Start the app with the given text as its ui.json, whatever it holds (a corrupt file, say).
pub fn harness_with_ui_json(name: &str, size: (f32, f32), snap: Snapshot, ui_json_text: &str, render: bool) -> H {
    let dir = tmp(name);
    let mock = dir.join("snapshot.json");
    std::fs::write(&mock, serde_json::to_vec(&snap).unwrap()).unwrap();
    let ui_json = dir.join("ui.json");
    std::fs::write(&ui_json, ui_json_text).unwrap();
    let opts = Opts { settings_path: Some(ui_json), no_watchers: true, ..Default::default() };
    let b = Harness::builder().with_size(egui::vec2(size.0, size.1));
    let b = if render { b.wgpu() } else { b };
    let mut h = b.build_ui_state(
        move |ui, app: &mut Option<App>| {
            if app.is_none() {
                *app = Some(App::with_ctx(ui.ctx(), None, Mode::Mock(mock.clone()), opts.clone()));
                return;
            }
            app.as_mut().unwrap().show(ui);
        },
        None,
    );
    h.run_steps(6);
    h
}

/// Like `harness` (without rendering) with a custom simulated time per step, for tests that need
/// clicks closer together than the default quarter second (double clicks).
pub fn harness_with_dt(name: &str, size: (f32, f32), snap: Snapshot, settings: Option<Settings>, step_dt: f32) -> H {
    let dir = tmp(name);
    let mock = dir.join("snapshot.json");
    std::fs::write(&mock, serde_json::to_vec(&snap).unwrap()).unwrap();
    let ui_json = dir.join("ui.json");
    if let Some(s) = with_theme(settings) {
        hwglance::settings::save_atomic(&ui_json, &s, false).unwrap();
    }
    let opts = Opts { settings_path: Some(ui_json), no_watchers: true, ..Default::default() };
    let mut h = Harness::builder().with_size(egui::vec2(size.0, size.1)).with_step_dt(step_dt).build_ui_state(
        move |ui, app: &mut Option<App>| {
            if app.is_none() {
                *app = Some(App::with_ctx(ui.ctx(), None, Mode::Mock(mock.clone()), opts.clone()));
                return;
            }
            app.as_mut().unwrap().show(ui);
        },
        None,
    );
    h.run_steps(6);
    h
}

pub fn app(h: &mut H) -> &mut App {
    h.state_mut().as_mut().unwrap()
}

pub fn save(h: &mut H, name: &str) {
    let Some(dir) = shots_dir() else { return };
    std::fs::create_dir_all(&dir).unwrap();
    let img = h.render().expect("render");
    img.save(dir.join(name)).unwrap();
}

pub fn click_secondary(h: &mut H, pos: egui::Pos2) {
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Secondary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        });
        h.run_steps(1);
    }
    h.run_steps(3);
}

pub fn click(h: &mut H, pos: egui::Pos2) {
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        });
        h.run_steps(1);
    }
    h.run_steps(3);
}

/// The wall clock of every demo scene (2026-10-13 09:00:00 UTC), so timestamps in the renders
/// ("Since 08:21", process start times) do not move with the day the tests run.
pub const SCENE_EPOCH_MS: u64 = 1_791_882_000_000;

/// A demo snapshot at demo time `t_end` after running the clock from `T_START - 1800`, plus the
/// history of every reading that the collector would hold.
pub fn demo_scene(t_end: f64) -> (Snapshot, Vec<hwglance_proto::HistorySeries>) {
    let mut d = Demo::at(SCENE_EPOCH_MS);
    d.warm_up(hwglance::demo::T_START - 1800.0, t_end, 5.0);
    let snap = d.snapshot(t_end);
    let ids: Vec<String> = snap
        .sensors
        .iter()
        .flat_map(|g| g.sections.iter())
        .flat_map(|s| s.readings.iter().map(|r| r.id.clone()).chain(s.summary.iter().map(|r| r.id.clone())))
        .collect();
    let hist = d.history_reply(&ids, 1800);
    (snap, hist)
}

pub fn repo_fixture() -> Snapshot {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/snapshot.json");
    let text = std::fs::read_to_string(p).expect("fixtures/snapshot.json");
    hwglance::conn::parse_mock(&text).unwrap()
}

/// Like `harness` for the live-data scene, with a reading to focus from the start (`--focus`).
pub fn harness_focus(name: &str, size: (f32, f32), snap: Snapshot, focus: &str) -> H {
    let dir = tmp(name);
    let mock = dir.join("snapshot.json");
    std::fs::write(&mock, serde_json::to_vec(&snap).unwrap()).unwrap();
    if let Some(s) = with_theme(None) {
        hwglance::settings::save_atomic(&dir.join("ui.json"), &s, false).unwrap();
    }
    let opts = Opts { settings_path: Some(dir.join("ui.json")), no_watchers: true, focus: Some(focus.to_string()), ..Default::default() };
    let mut h = Harness::builder().with_size(egui::vec2(size.0, size.1)).build_ui_state(
        move |ui, app: &mut Option<App>| {
            if app.is_none() {
                *app = Some(App::with_ctx(ui.ctx(), None, Mode::Mock(mock.clone()), opts.clone()));
                return;
            }
            app.as_mut().unwrap().show(ui);
        },
        None,
    );
    h.run_steps(8);
    h
}

/// The `expanded` process view in both docks.
pub fn expanded_view(mut s: Settings) -> Settings {
    s.layout.process_view.bottom = "expanded".into();
    s.layout.process_view.right = "expanded".into();
    s
}

/// A process-panel scene: the busiest multi-process app expanded and selected, the pointer parked
/// away from every row.
pub fn procs_harness(name: &str, size: (f32, f32), settings: Settings) -> H {
    let (snap, _) = demo_scene(hwglance::demo::T_START + 30.0);
    let mut h = harness(name, size, snap, Some(settings), true, false);
    {
        let a = app(&mut h);
        let id = a
            .groups()
            .iter()
            .filter(|g| g.procs.len() > 3 && g.class == hwglance_proto::ProcClass::App)
            .max_by(|x, y| x.cpu_pct.total_cmp(&y.cpu_pct))
            .map(|g| g.id.clone())
            .unwrap();
        a.expanded.insert(id.clone());
        a.selection = Some(hwglance::model::RowId::Group(id));
        a.rows_dirty = true;
    }
    h.run_steps(3);
    h.hover_at(egui::pos2(600.0, 5.0));
    h.run_steps(3);
    h
}

/// How far the panels are scrolled in the history popover scene.
const SCROLL_PX: f32 = 200.0;

/// The history popover: panels scrolled until the header of card `head` is pinned, then the
/// history of its row `reading` pinned open.
pub fn popover_harness(name: &str, size: (f32, f32), settings: Option<Settings>, head: &str, reading: &str) -> H {
    let (snap, hist) = demo_scene(hwglance::demo::T_START + 140.0);
    let mut h = harness(name, size, snap, settings, true, false);
    let ctx = h.ctx.clone();
    app(&mut h).link.publish_history(&ctx, hist);
    h.hover_at(egui::pos2(900.0, 300.0));
    h.run_steps(2);
    h.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: egui::vec2(0.0, -SCROLL_PX),
        modifiers: egui::Modifiers::NONE,
        phase: egui::TouchPhase::Move,
    });
    h.run_steps(4);
    assert!(app(&mut h).sticky_heads.contains(&head.to_string()), "pinned: {:?}", app(&mut h).sticky_heads);
    app(&mut h).pinned = Some(reading.into());
    h.hover_at(egui::pos2(5.0, 5.0));
    h.run_steps(5);
    h
}

/// Three watched readings; with `watched_snapshot` two of them carry limits of the user's, so that
/// every kind of cell colour and flag shows up.
pub fn watched_settings() -> Settings {
    let mut s = Settings::default();
    s.sensors.highlighted = ["cpu/temp/tctl", "gpu0/load/core", "gpu0/temp/hotspot"].iter().map(|s| s.to_string()).collect();
    s
}

/// The snapshot as the collector sends it when the user's limits file changes some readings: their
/// warn and crit are the limits in force, and `limit_overrides` remembers the defaults.
pub fn with_limits(mut snap: Snapshot, limits: &[(&str, Option<f64>, Option<f64>, bool)]) -> Snapshot {
    for g in &mut snap.sensors {
        for s in &mut g.sections {
            for r in s.readings.iter_mut().chain(s.summary.iter_mut()) {
                if let Some(&(id, warn, crit, below)) = limits.iter().find(|l| l.0 == r.id) {
                    snap.limit_overrides.insert(
                        id.to_string(),
                        hwglance_proto::LimitOverride { warn, crit, below, default_warn: r.warn, default_crit: r.crit },
                    );
                    (r.warn, r.crit) = (warn, crit);
                }
            }
        }
    }
    snap
}

/// `watched_settings` goes with this: Tctl warns at 50 and is critical at 75, the GPU hotspot warns at 40.
pub fn watched_snapshot(snap: Snapshot) -> Snapshot {
    with_limits(snap, &[("cpu/temp/tctl", Some(50.0), Some(75.0), false), ("gpu0/temp/hotspot", Some(40.0), Some(95.0), false)])
}

/// No daemon: skeleton panels, the waiting message and the banner of a collector that is not
/// running. Points the live connection at an empty runtime directory.
pub fn connecting_harness(size: (f32, f32), settings: Option<Settings>) -> H {
    let dir = std::env::temp_dir().join("hwglance-no-daemon-3");
    std::fs::create_dir_all(&dir).unwrap();
    // SAFETY: only the connecting scene changes the variable, and no other test of its binary reads it.
    unsafe { std::env::set_var("XDG_RUNTIME_DIR", &dir) };
    let down = tmp("down").join("ui.json");
    if let Some(s) = with_theme(settings) {
        hwglance::settings::save_atomic(&down, &s, false).unwrap();
    }
    let opts = Opts { settings_path: Some(down), no_watchers: true, ..Default::default() };
    let mut h = Harness::builder().with_size(egui::vec2(size.0, size.1)).wgpu().build_ui_state(
        move |ui, app: &mut Option<App>| {
            if app.is_none() {
                *app = Some(App::with_ctx(ui.ctx(), None, Mode::Live, opts.clone()));
                return;
            }
            app.as_mut().unwrap().show(ui);
        },
        None,
    );
    for _ in 0..6 {
        std::thread::sleep(std::time::Duration::from_millis(150));
        h.run_steps(2);
    }
    h
}

/// A wide-screen sensor arrangement: four explicit columns of 496 px, which stack into fewer display
/// columns when the window or the process panel leaves less room.
pub fn users_arrangement(dock: hwglance::settings::Dock, view: &str, width_frac: Option<f32>) -> Settings {
    use hwglance::settings::ColumnPref;
    let col = |ids: &[&str]| ColumnPref { width: 496.0, cards: ids.iter().map(|s| s.to_string()).collect() };
    let mut s = Settings::default();
    s.layout.auto = false;
    s.layout.columns = vec![col(&["cpu", "memory"]), col(&["gpu0"]), col(&["mb", "igpu"]), col(&["nvme0", "nvme1", "nvme2"])];
    s.layout.set_dock(dock);
    s.layout.process_view.right = view.into();
    if let Some(f) = width_frac {
        s.processes.views.right_expanded.size = Some(f);
    }
    s
}

/// The demo scene with the given settings and the layout editor on or off.
pub fn arrangement_harness(name: &str, size: (f32, f32), s: Settings, edit: bool) -> H {
    let (snap, _) = demo_scene(hwglance::demo::T_START + 30.0);
    let mut h = harness(name, size, snap, Some(s), true, false);
    app(&mut h).edit_layout = edit;
    h.run_steps(4);
    h
}

/// Window title of the Arabic chat group: "WhatsApp - chat with Ahmad".
pub const ARABIC_TITLE: &str = "واتساب - محادثة مع أحمد";
/// Name of the Arabic-named app: "Notepad".
pub const ARABIC_APP: &str = "المفكرة";
pub const ARABIC_APP_ID: &str = "app:arabic-notes";

/// Add two Arabic app groups to the mock snapshot, busy enough to sit near the top of the
/// list: a Latin-named chat app whose window title is Arabic, and an app named in Arabic with
/// several processes that are named in Arabic too.
pub fn with_arabic_groups(mut snap: Snapshot) -> Snapshot {
    let procs = snap.procs.as_mut().expect("the demo snapshot has processes");
    let template = procs.iter().find(|g| g.id == "app:discord").expect("discord group").clone();
    let busiest = procs.iter().map(|g| g.cpu_pct).fold(0.0_f32, f32::max);
    let make = |id: &str, name: &str, title: &str, proc_names: &[&str], cpu: f32, pid0: i32| {
        let mut g = template.clone();
        g.id = id.into();
        g.name = name.into();
        g.icon = None;
        g.title = Some(title.into());
        g.procs.truncate(1);
        let first = g.procs[0].clone();
        g.procs = proc_names
            .iter()
            .enumerate()
            .map(|(i, n)| {
                let mut p = first.clone();
                p.pid = pid0 + i as i32;
                p.name = (*n).into();
                p.cmdline = format!("/usr/bin/{n} --profile \"{title}\"");
                p.cpu_pct = cpu / proc_names.len() as f32;
                p
            })
            .collect();
        g.cpu_pct = cpu;
        g.mem_bytes = g.procs.iter().map(|p| p.mem_bytes).sum();
        g
    };
    procs.push(make("app:whatsapp", "WhatsApp", ARABIC_TITLE, &["whatsapp"], busiest * 0.9, 61000));
    procs.push(make(ARABIC_APP_ID, ARABIC_APP, "ملاحظات - مشروع التقرير 2024 - مراجعة الميزانية السنوية والخطة التشغيلية للفريق في الربع القادم مع الإدارة", &["المفكرة", "مدير الملفات", "notes-sync"], busiest * 0.8, 62000));
    snap
}

/// The process panel on a scene with the Arabic groups, the Arabic-named app expanded.
pub fn arabic_procs_harness(name: &str, size: (f32, f32), settings: Settings) -> H {
    let (snap, _) = demo_scene(hwglance::demo::T_START + 30.0);
    let mut h = harness(name, size, with_arabic_groups(snap), Some(settings), true, false);
    {
        let a = app(&mut h);
        a.expanded.insert(ARABIC_APP_ID.into());
        a.rows_dirty = true;
    }
    h.run_steps(3);
    h.hover_at(egui::pos2(600.0, 5.0));
    h.run_steps(3);
    h
}
