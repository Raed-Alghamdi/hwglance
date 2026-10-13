//! Offscreen renders of the real UI with egui_kittest (no window). Writes PNGs into the
//! directory named by `HWGLANCE_SHOTS`; without it the tests only check that rendering works.

mod common;

use common::*;
use egui::pos2;
use hwglance::model::RowId;
use egui_kittest::kittest::Queryable;
use hwglance::settings::{self, Settings};

const SIZE: (f32, f32) = (2048.0, 1150.0);
/// The maximized window on the right monitor (2560x1440 at 106.67 %).
const WIDE: (f32, f32) = (2400.0, 1350.0);
/// `HWGLANCE_SHOT_SIZE=2546x1400` renders every shot at that size instead of its own.
fn sized(default: (f32, f32)) -> (f32, f32) {
    let Ok(v) = std::env::var("HWGLANCE_SHOT_SIZE") else { return default };
    let (w, h) = v.split_once('x').expect("WxH");
    (w.parse().unwrap(), h.parse().unwrap())
}

#[test]
fn shot_01_default_real_fixture() {
    let mut h = harness("default", sized(SIZE), repo_fixture(), None, true, false);
    save(&mut h, "01-default.png");
}

#[test]
fn shot_02_watched_values() {
    let mut h = harness("watched", sized(SIZE), watched_snapshot(repo_fixture()), Some(watched_settings()), true, false);
    save(&mut h, "02-watched.png");
}

#[test]
fn shot_03_history_popover() {
    let mut h = popover_harness("popover", sized(SIZE), None, "gpu0", "gpu0/power/board");
    save(&mut h, "03-history-popover.png");
}

#[test]
fn shot_03b_history_of_a_low_series() {
    // A series that hovers near zero (GPU memory controller): the plot keeps its own gutters and
    // the line is not glued to an axis.
    let (snap, _) = demo_scene(hwglance::demo::T_START + 140.0);
    let mut h = harness("popover-low", sized(SIZE), snap.clone(), None, true, false);
    let ctx = h.ctx.clone();
    let points: Vec<(u64, f32)> = (0..360)
        .map(|i| {
            let t = snap.unix_ms - 1_800_000 + i as u64 * 5000;
            let burst = if (200..230).contains(&i) { 12.0 + (i % 5) as f32 * 1.6 } else { 0.0 };
            (t, (burst + ((i * 7) % 5) as f32 * 0.15).min(20.0))
        })
        .collect();
    let series = hwglance_proto::HistorySeries { id: "gpu0/load/memctl".into(), unit: hwglance_proto::Unit::Percent, points };
    app(&mut h).link.publish_history(&ctx, vec![series]);
    app(&mut h).pinned = Some("gpu0/load/memctl".into());
    h.hover_at(pos2(5.0, 5.0));
    h.run_steps(5);
    save(&mut h, "03b-history-low.png");
}

#[test]
fn shot_04_throttling_and_collapsed() {
    let (snap, _) = demo_scene(hwglance::demo::T_START + 362.0);
    let mut s = Settings::default();
    for id in ["cpu/clock", "cpu/power", "gpu0/temp", "gpu0/clocks", "memory/temp"] {
        s.layout.collapsed_sections.insert(id.into(), true);
    }
    s.layout.collapsed_cards.push("memory".into());
    s.sensors.summary_overrides.insert("cpu/clock".into(), "cpu/clock/core0".into());
    let mut h = harness("throttle", sized(SIZE), snap, Some(s), true, false);
    save(&mut h, "04-throttling-and-collapsed.png");
}

#[test]
fn shot_19_watched_summary_of_a_collapsed_section_and_card() {
    // Tctl is watched and is the summary of the collapsed CPU temperatures section; then the CPU
    // card is collapsed to its chips, the first of them that watched summary
    let (snap, _) = demo_scene(hwglance::demo::T_START + 30.0);
    let snap = watched_snapshot(snap);
    let mut s = watched_settings();
    for id in ["cpu/temp", "cpu/clock", "gpu0/temp"] {
        s.layout.collapsed_sections.insert(id.into(), true);
    }
    s.sensors.summary_overrides.insert("cpu/temp".into(), "cpu/temp/tctl".into());
    let mut h = harness("watched-collapsed", sized(SIZE), snap.clone(), Some(s.clone()), true, false);
    save(&mut h, "19-watched-collapsed-summary.png");
    s.layout.collapsed_cards.push("cpu".into());
    let mut h = harness("watched-collapsed-card", sized(SIZE), snap, Some(s), true, false);
    save(&mut h, "19b-watched-collapsed-card-chips.png");
}

/// A process-panel scene: the busiest multi-process app expanded and selected.
fn procs_scene(name: &str, settings: Settings, file: &str) {
    let mut h = procs_harness(name, sized(WIDE), settings);
    save(&mut h, file);
}

#[test]
fn shot_05_processes_bottom_compact() {
    procs_scene("procs", Settings::default(), "05-processes.png");
}

#[test]
fn shot_05b_processes_bottom_expanded() {
    procs_scene("procs-expanded", expanded_view(Settings::default()), "05b-processes-expanded.png");
}

#[test]
fn shot_06_settings_alerts() {
    let mut h = harness("alerts", sized((1700.0, 1000.0)), repo_fixture(), None, true, true);
    app(&mut h).settings_tab = hwglance::app::SettingsTab::Alerts;
    h.run_steps(4);
    save(&mut h, "06-settings-alerts.png");
}

#[test]
fn shot_06b_settings_general_appearance() {
    let mut h = harness("general", sized((1700.0, 1000.0)), repo_fixture(), None, true, true);
    app(&mut h).settings_tab = hwglance::app::SettingsTab::General;
    h.run_steps(4);
    save(&mut h, "06b-settings-general.png");
}

#[test]
fn shot_07_small_window() {
    let mut h = harness("small", (1200.0, 700.0), repo_fixture(), None, true, false);
    save(&mut h, "07-small-1200x700.png");
}

#[test]
fn shot_08_edit_layout_mode() {
    let mut h = harness("edit", sized(SIZE), repo_fixture(), None, true, false);
    app(&mut h).edit_layout = true;
    h.run_steps(3);
    save(&mut h, "08-edit-layout.png");
}

#[test]
fn shot_10_connecting_and_collector_down() {
    let mut h = connecting_harness(sized((1400.0, 600.0)), None);
    // The message names the state and carries the action; no "waiting" text for a dead collector.
    assert!(h.query_by_label("Collector not running").is_some());
    assert!(h.query_by_label("Start collector").is_some());
    assert!(h.query_by_label_contains("Waiting").is_none());
    if let Some(dir) = shots_dir() {
        std::fs::create_dir_all(&dir).unwrap();
        h.render().unwrap().save(dir.join("10-connecting.png")).unwrap();
    }
}

#[test]
fn shot_11_force_kill_confirmation() {
    let mut h = harness("kill", sized(SIZE), repo_fixture(), None, true, false);
    let id = app(&mut h).groups().iter().find(|g| g.procs.len() > 3).unwrap().id.clone();
    app(&mut h).selection = Some(RowId::Group(id));
    app(&mut h).request_kill(None, hwglance::actions::Signal::Kill);
    h.run_steps(4);
    save(&mut h, "11-force-kill.png");
}

#[test]
fn shot_12_waiting_for_the_first_snapshot() {
    // A source that has not produced a snapshot yet: skeleton panels in the saved layout's columns.
    let dir = common::tmp("waiting");
    let mut s = Settings::default();
    s.layout.auto = false;
    s.layout.columns = vec![
        hwglance::settings::ColumnPref { width: 461.0, cards: vec!["cpu".into(), "memory".into()] },
        hwglance::settings::ColumnPref { width: 461.0, cards: vec!["gpu0".into()] },
        hwglance::settings::ColumnPref { width: 461.0, cards: vec!["board".into(), "igpu".into()] },
        hwglance::settings::ColumnPref { width: 461.0, cards: vec!["disk/a".into(), "disk/b".into()] },
    ];
    hwglance::settings::save_atomic(&dir.join("ui.json"), &with_theme(Some(s)).unwrap(), false).unwrap();
    let opts = hwglance::app::Opts { settings_path: Some(dir.join("ui.json")), no_watchers: true, ..Default::default() };
    let missing = dir.join("missing.json");
    let mut h = egui_kittest::Harness::builder().with_size({ let (w, h) = sized((2048.0, 700.0)); egui::vec2(w, h) }).wgpu().build_ui_state(
        move |ui, app: &mut Option<hwglance::app::App>| {
            if app.is_none() {
                *app = Some(hwglance::app::App::with_ctx(ui.ctx(), None, hwglance::app::Mode::Mock(missing.clone()), opts.clone()));
                return;
            }
            app.as_mut().unwrap().show(ui);
        },
        None,
    );
    h.run_steps(6);
    if let Some(dir) = shots_dir() {
        std::fs::create_dir_all(&dir).unwrap();
        h.render().unwrap().save(dir.join("12-waiting.png")).unwrap();
    }
}

#[test]
fn shot_13_dock_right_compact() {
    let mut s = Settings::default();
    s.layout.set_dock(hwglance::settings::Dock::Right);
    procs_scene("dock-right", s, "13-dock-right.png");
}

#[test]
fn shot_13b_dock_right_expanded() {
    let mut s = Settings::default();
    s.layout.set_dock(hwglance::settings::Dock::Right);
    procs_scene("dock-right-expanded", expanded_view(s), "13b-dock-right-expanded.png");
}

#[test]
fn shot_14_columns_menu() {
    let (snap, _) = demo_scene(hwglance::demo::T_START + 30.0);
    let mut h = harness("columns-menu", sized(WIDE), snap, Some(expanded_view(Settings::default())), true, false);
    let list = app(&mut h).procs_list_rect;
    click_secondary(&mut h, pos2(list.left() + 700.0, list.top() - 14.0));
    save(&mut h, "14-columns-menu.png");
}

/// A wide-screen sensor arrangement: four columns of 496 shared by every dock and view.
const LAYOUT_SIZE: (f32, f32) = (2387.0, 1312.0);

fn layout_scene(name: &str, s: Settings, edit: bool, file: &str) {
    let mut h = arrangement_harness(name, LAYOUT_SIZE, s, edit);
    let mut cols: Vec<f32> = app(&mut h).card_rects.iter().map(|(_, r)| (r.left() / 4.0).round() * 4.0).collect();
    cols.dedup();
    eprintln!("{file}: card lefts {cols:?}, panel {:.0} px", app(&mut h).procs_list_rect.width() + 24.0);
    save(&mut h, file);
}

#[test]
fn shot_15_shared_arrangement_bottom_compact() {
    layout_scene("lay-bottom", users_arrangement(hwglance::settings::Dock::Bottom, "compact", None), false, "15a-bottom-compact.png");
}

#[test]
fn shot_15_shared_arrangement_right_compact() {
    let mut s = users_arrangement(hwglance::settings::Dock::Right, "compact", None);
    s.processes.views.right_compact.width_px = Some(340.0);
    layout_scene("lay-right-compact", s, false, "15b-right-compact-340px.png");
}

#[test]
fn shot_15_shared_arrangement_right_expanded() {
    let s = users_arrangement(hwglance::settings::Dock::Right, "expanded", Some(0.474));
    layout_scene("lay-right-expanded", s, false, "15c-right-expanded.png");
}

#[test]
fn shot_15_shared_arrangement_right_expanded_edit() {
    let s = users_arrangement(hwglance::settings::Dock::Right, "expanded", Some(0.474));
    layout_scene("lay-right-expanded-edit", s, true, "15d-right-expanded-edit.png");
}

#[test]
fn shot_15_shared_arrangement_right_compact_500px() {
    let mut s = users_arrangement(hwglance::settings::Dock::Right, "compact", None);
    s.processes.views.right_compact.width_px = Some(500.0);
    layout_scene("lay-right-compact-500", s, false, "15e-right-compact-500px.png");
}

/// A real-size window.
const REAL: (f32, f32) = (2546.0, 1400.0);

/// Like `procs_scene` at the real window size, returning the harness for further steps.
fn name_col_scene(name: &str, settings: Settings) -> H {
    let (snap, _) = demo_scene(hwglance::demo::T_START + 30.0);
    let mut h = harness(name, REAL, snap, Some(settings), true, false);
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
        a.selection = Some(RowId::Group(id));
        a.rows_dirty = true;
    }
    h.run_steps(3);
    h
}

fn drag_px(h: &mut H, from: egui::Pos2, dx: f32) {
    let to = from + egui::vec2(dx, 0.0);
    h.event(egui::Event::PointerMoved(from));
    h.run_steps(2);
    h.event(egui::Event::PointerButton { pos: from, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE });
    h.run_steps(2);
    for i in 1..=10 {
        h.event(egui::Event::PointerMoved(from + (to - from) * (i as f32 / 10.0)));
        h.run_steps(2);
    }
    h.event(egui::Event::PointerButton { pos: to, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE });
    h.run_steps(4);
}

fn divider_pos(h: &mut H) -> egui::Pos2 {
    let (list, x) = (app(h).procs_list_rect, app(h).name_divider_x);
    pos2(x, list.top() - 14.0)
}

#[test]
fn shot_16_name_column_right_compact_default_then_name_narrowed() {
    let mut s = Settings::default();
    s.layout.set_dock(hwglance::settings::Dock::Right);
    let mut h = name_col_scene("name-right", s);
    h.hover_at(pos2(600.0, 5.0));
    h.run_steps(3);
    save(&mut h, "16a-right-compact-default.png");
    let before = app(&mut h).procs_list_rect.width();
    // the divider moves the boundary between Name and the data columns; the panel keeps its size
    let from = divider_pos(&mut h);
    drag_px(&mut h, from, -90.0);
    let after = app(&mut h).procs_list_rect.width();
    assert_eq!(before, after, "the panel did not resize");
    h.hover_at(pos2(600.0, 5.0));
    h.run_steps(3);
    save(&mut h, "16b-right-compact-name-narrowed-90.png");
    // widening the panel afterwards gives the width to Name only; the data columns keep theirs
    app(&mut h).s.processes.views.right_compact.width_px = Some(settings::COMPACT_WIDTH_PX + 140.0);
    h.run_steps(4);
    assert!(app(&mut h).procs_list_rect.width() > before + 100.0, "the panel widened");
    h.hover_at(pos2(600.0, 5.0));
    h.run_steps(3);
    save(&mut h, "16d-right-compact-panel-widened-140.png");
    // mid-drag in the bottom dock: the divider is lit and a guide runs down the list
    let mut b = name_col_scene("name-guide", Settings::default());
    let from = divider_pos(&mut b);
    b.event(egui::Event::PointerMoved(from));
    b.run_steps(2);
    b.event(egui::Event::PointerButton { pos: from, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE });
    b.run_steps(2);
    for i in 1..=6 {
        b.event(egui::Event::PointerMoved(from + egui::vec2(-12.0 * i as f32, 0.0)));
        b.run_steps(2);
    }
    save(&mut b, "16c-bottom-mid-drag-guide.png");
}

#[test]
fn shot_17_name_column_bottom_compact_260() {
    let mut s = Settings::default();
    s.processes.views.bottom_compact.name_width = Some(260.0);
    let mut h = name_col_scene("name-bottom", s);
    h.hover_at(pos2(600.0, 5.0));
    h.run_steps(3);
    save(&mut h, "17-bottom-compact-name-260.png");
}

#[test]
fn shot_18_settings_bar_button_option() {
    let mut s = Settings::default();
    s.tray.bar_display = "cpu_gpu".into();
    let mut h = harness("bar-option", REAL, repo_fixture(), Some(s), true, true);
    app(&mut h).settings_tab = hwglance::app::SettingsTab::BarWidget;
    h.run_steps(4);
    save(&mut h, "18-settings-bar-button.png");
}

#[test]
fn shot_20_window_controls_close_hover_and_focus_in_both_palettes() {
    use egui::accesskit::Role;
    for theme in ["dark", "light"] {
        let mut s = Settings::default();
        s.appearance.theme = theme.into();
        let mut h = harness("wc-shot", REAL, repo_fixture(), Some(s), true, false);
        // maximized: the first button says Restore; the keyboard is on it
        h.input_mut().viewports.get_mut(&egui::ViewportId::ROOT).unwrap().maximized = Some(true);
        h.run_steps(2);
        h.get_by_role_and_label(Role::Button, "Restore").focus();
        h.run_steps(3);
        save(&mut h, &format!("20a-window-controls-restore-focused-{theme}.png"));
        // the pointer on Close: the critical wash
        let at = h.get_by_role_and_label(Role::Button, "Close").rect().center();
        h.get_by_role_and_label(Role::Button, "Settings").focus();
        h.hover_at(at);
        h.run_steps(3);
        save(&mut h, &format!("20b-window-controls-close-hover-{theme}.png"));
    }
}
