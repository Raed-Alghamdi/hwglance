//! Evidence pack for a UI review: offscreen renders, task walks and measured checks. Every test
//! is `#[ignore]`d so `cargo test` stays fast; run them one thread at a time with
//! `HWGLANCE_EVIDENCE=<run folder> cargo test -p hwglance --test evidence -- --ignored --test-threads=1`.
//! Nothing here opens a window: the harness renders offscreen (wgpu, no surface), and the walks
//! change every pid of their snapshot to one that does not exist, so no signal can reach a real
//! process.

mod common;

use common::*;
use egui::{pos2, Key};
use egui_kittest::kittest::{NodeT, Queryable};
use hwglance::app::SettingsTab;
use hwglance::settings::{Dock, Settings};
use std::fmt::Write as _;
use std::path::PathBuf;

/// The run folder (`gates/` and `walk/` live under it). Without it the tests render but write nothing.
fn run_dir() -> Option<PathBuf> {
    std::env::var_os("HWGLANCE_EVIDENCE").map(PathBuf::from)
}

fn out_path(sub: &str, name: &str) -> Option<PathBuf> {
    let p = run_dir()?.join(sub).join(name);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    Some(p)
}

fn shot_to(h: &mut H, sub: &str, name: &str) {
    let Some(p) = out_path(sub, name) else { return };
    h.render().expect("render").save(p).unwrap();
}

fn write_text(sub: &str, name: &str, text: &str) {
    if let Some(p) = out_path(sub, name) {
        std::fs::write(p, text).unwrap();
    }
}

fn themed(theme: &str, mut s: Settings) -> Settings {
    s.appearance.theme = theme.into();
    s
}

/// Replace every pid of the snapshot by one above any live pid and an impossible start time, so
/// `actions::execute` finds each process "already gone" and never sends a signal (the check
/// `still_same` reads /proc/<pid>/stat first). Asserts that no such pid exists right now.
fn unkillable(mut snap: hwglance_proto::Snapshot) -> hwglance_proto::Snapshot {
    let mut next = 4_190_000;
    for g in snap.procs.iter_mut().flatten() {
        for p in &mut g.procs {
            next += 1;
            assert!(!std::path::Path::new(&format!("/proc/{next}")).exists(), "pid {next} is live");
            p.pid = next;
            p.ppid = 1;
            p.start_time_ticks = u64::MAX;
        }
    }
    snap
}

const SIZES: [(&str, (f32, f32)); 3] = [("2546x1400", (2546.0, 1400.0)), ("2386x1302", (2386.0, 1302.0)), ("1269x1400", (1269.0, 1400.0))];
const SMALL: (f32, f32) = (1200.0, 700.0);

fn first_row_pos(card: egui::Rect) -> egui::Pos2 {
    pos2(card.left() + 100.0, card.top() + 36.0 + 20.0 + 28.0 + 14.0)
}

/// The window waiting on a source that has not produced a snapshot yet.
fn waiting_harness(size: (f32, f32), settings: Settings) -> H {
    let dir = tmp("ev-waiting");
    hwglance::settings::save_atomic(&dir.join("ui.json"), &settings, false).unwrap();
    let opts = hwglance::app::Opts { settings_path: Some(dir.join("ui.json")), no_watchers: true, ..Default::default() };
    let missing = dir.join("missing.json");
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(size.0, size.1)).wgpu().build_ui_state(
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
    h
}

/// A history popover pinned on `reading`, the panels scrolled to it (what `--focus` does, minus the flash).
fn popover_state(name: &str, size: (f32, f32), settings: Settings, reading: &str) -> H {
    let (snap, hist) = demo_scene(hwglance::demo::T_START + 140.0);
    let mut h = harness(name, size, snap, Some(settings), true, false);
    let ctx = h.ctx.clone();
    app(&mut h).link.publish_history(&ctx, hist);
    app(&mut h).apply_focus(&ctx, reading, false);
    h.run_steps(6);
    app(&mut h).flash = None;
    h.hover_at(pos2(5.0, 5.0));
    h.run_steps(4);
    h
}

fn state_names() -> Vec<&'static str> {
    vec![
        "default",
        "watched",
        "history-popover",
        "throttling-collapsed",
        "processes-expanded",
        "force-kill-confirm",
        "dock-right-compact",
        "dock-right-expanded",
        "edit-layout",
        "settings-general",
        "settings-sensors",
        "settings-layout",
        "settings-processes",
        "settings-history",
        "settings-alerts",
        "settings-bar-widget",
        "settings-collector",
        "value-menu",
        "collector-not-running",
        "waiting-first-data",
    ]
}

fn build(state: &str, theme: &str, size: (f32, f32)) -> H {
    let t = |s: Settings| themed(theme, s);
    let n = format!("ev-{state}");
    let tab = |tab: SettingsTab| {
        let mut h = harness(&n, size, repo_fixture(), Some(t(Settings::default())), true, true);
        app(&mut h).settings_tab = tab;
        h.run_steps(4);
        h
    };
    match state {
        "default" => harness(&n, size, repo_fixture(), Some(t(Settings::default())), true, false),
        "watched" => harness(&n, size, watched_snapshot(repo_fixture()), Some(t(watched_settings())), true, false),
        "history-popover" => popover_state(&n, size, t(Settings::default()), "gpu0/power/board"),
        "throttling-collapsed" => {
            let (snap, _) = demo_scene(hwglance::demo::T_START + 362.0);
            let mut s = Settings::default();
            for id in ["cpu/clock", "cpu/power", "gpu0/temp", "gpu0/clocks", "memory/temp"] {
                s.layout.collapsed_sections.insert(id.into(), true);
            }
            s.layout.collapsed_cards.push("memory".into());
            s.sensors.summary_overrides.insert("cpu/clock".into(), "cpu/clock/core0".into());
            harness(&n, size, snap, Some(t(s)), true, false)
        }
        "processes-expanded" => procs_harness(&n, size, t(expanded_view(Settings::default()))),
        "force-kill-confirm" => {
            let (snap, _) = demo_scene(hwglance::demo::T_START + 30.0);
            let mut h = harness(&n, size, snap, Some(t(Settings::default())), true, false);
            let id = app(&mut h).groups().iter().find(|g| g.procs.len() > 3).unwrap().id.clone();
            app(&mut h).selection = Some(hwglance::model::RowId::Group(id));
            app(&mut h).request_kill(None, hwglance::actions::Signal::Kill);
            h.run_steps(4);
            h
        }
        "dock-right-compact" => {
            let mut s = t(Settings::default());
            s.layout.set_dock(Dock::Right);
            procs_harness(&n, size, s)
        }
        "dock-right-expanded" => {
            let mut s = t(Settings::default());
            s.layout.set_dock(Dock::Right);
            procs_harness(&n, size, expanded_view(s))
        }
        "edit-layout" => {
            let mut h = harness(&n, size, repo_fixture(), Some(t(Settings::default())), true, false);
            app(&mut h).edit_layout = true;
            h.run_steps(3);
            h
        }
        "settings-general" => tab(SettingsTab::General),
        "settings-sensors" => tab(SettingsTab::Sensors),
        "settings-layout" => tab(SettingsTab::Layout),
        "settings-processes" => tab(SettingsTab::Processes),
        "settings-history" => tab(SettingsTab::History),
        "settings-alerts" => tab(SettingsTab::Alerts),
        "settings-bar-widget" => tab(SettingsTab::BarWidget),
        "settings-collector" => tab(SettingsTab::Collector),
        "value-menu" => {
            let (snap, _) = demo_scene(hwglance::demo::T_START + 30.0);
            let mut h = harness(&n, size, snap, Some(t(Settings::default())), true, false);
            let cpu = app(&mut h).card_rects.iter().find(|(id, _)| id == "cpu").unwrap().1;
            click_secondary(&mut h, first_row_pos(cpu));
            h
        }
        "collector-not-running" => connecting_harness(size, Some(t(Settings::default()))),
        "waiting-first-data" => {
            let mut s = Settings::default();
            s.layout.auto = false;
            s.layout.columns = vec![
                hwglance::settings::ColumnPref { width: 461.0, cards: vec!["cpu".into(), "memory".into()] },
                hwglance::settings::ColumnPref { width: 461.0, cards: vec!["gpu0".into()] },
                hwglance::settings::ColumnPref { width: 461.0, cards: vec!["board".into(), "igpu".into()] },
            ];
            waiting_harness(size, t(s))
        }
        other => panic!("unknown state {other}"),
    }
}

/// Every main state in both themes at three window sizes, and the default state at 1200x700.
#[test]
#[ignore]
fn evidence_renders() {
    let _g = layout_lock();
    let only = std::env::var("HWGLANCE_ONLY").ok();
    let mut report = String::new();
    for state in state_names() {
        if only.as_deref().is_some_and(|o| o != state) {
            continue;
        }
        for theme in ["dark", "light"] {
            let mut sizes: Vec<(&str, (f32, f32))> = SIZES.to_vec();
            if state == "default" {
                sizes.push(("1200x700", SMALL));
            }
            for (label, size) in sizes {
                let mut h = build(state, theme, size);
                let name = format!("{state}_{theme}_{label}.png");
                shot_to(&mut h, "gates/shots", &name);
                let _ = writeln!(report, "{name}");
            }
        }
    }
    write_text("gates/data", "shots-list.txt", &report);
}

// ---------------------------------------------------------------------------------------------
// Task walks: one screenshot per step, dark theme, the 4K monitor's window (2546x1400)
// ---------------------------------------------------------------------------------------------

const REAL: (f32, f32) = (2546.0, 1400.0);

struct Walk {
    dir: String,
    n: usize,
    log: String,
}

impl Walk {
    fn new(task: &str) -> Walk {
        Walk { dir: format!("walk/{task}"), n: 0, log: String::new() }
    }

    /// Write `NN-<step>.png` and note what the state is, for the walk's own log.
    fn step(&mut self, h: &mut H, step: &str, note: &str) {
        self.n += 1;
        let name = format!("{:02}-{step}.png", self.n);
        shot_to(h, &self.dir, &name);
        let _ = writeln!(self.log, "{name}: {note}");
    }

    fn finish(self) {
        write_text(&self.dir, "steps.txt", &self.log);
    }
}

fn park(h: &mut H) {
    h.hover_at(pos2(1500.0, 3.0));
    h.run_steps(3);
}

fn dark(s: Settings) -> Settings {
    themed("dark", s)
}

/// The nodes of the process table header, left to right, without the full-width background and
/// the 8 px Name divider.
fn header_cells(h: &H, list_top: f32) -> Vec<egui::Rect> {
    let mut v: Vec<egui::Rect> = h
        .root()
        .children_recursive()
        .filter_map(|n| n.accesskit_node().bounding_box().map(|_| n.rect()))
        .filter(|r| (r.bottom() - list_top).abs() < 1.0 && r.width() > 20.0 && r.width() < 600.0)
        .collect();
    v.sort_by(|a, b| a.min.x.total_cmp(&b.min.x));
    v
}

#[test]
#[ignore]
fn walk_t1_end_a_resource_hog() {
    let _g = layout_lock();
    let (snap, _) = demo_scene(hwglance::demo::T_START + 30.0);
    let snap = unkillable(snap);
    let mut h = harness("ev-t1", REAL, snap, Some(dark(Settings::default())), true, false);
    let mut w = Walk::new("T1");
    park(&mut h);
    w.step(&mut h, "window-as-opened", "bottom dock, process table already sorted by CPU, descending; nothing selected");
    assert_eq!(app(&mut h).s.processes.sort.column, "cpu");

    // Click the CPU header: it is the active column, so the order flips.
    let list_top = app(&mut h).procs_list_rect.top();
    let cells = header_cells(&h, list_top);
    let cpu = cells[2];
    click(&mut h, cpu.center());
    park(&mut h);
    let desc_after_first = app(&mut h).s.processes.sort.descending;
    w.step(&mut h, "click-cpu-header", &format!("clicking the active CPU header flips the order; descending is now {desc_after_first}"));
    click(&mut h, cpu.center());
    park(&mut h);
    let desc = app(&mut h).s.processes.sort.descending;
    w.step(&mut h, "click-cpu-header-again", &format!("second click restores descending order ({desc})"));

    // Select the top app: the first group row under the Apps section row.
    let list = app(&mut h).procs_list_rect;
    let row = pos2(list.left() + 140.0, list.top() + 28.0 + 14.0);
    click(&mut h, row);
    park(&mut h);
    let sel = app(&mut h).selected_target().map(|t| (t.name, t.procs.len()));
    w.step(&mut h, "select-top-app", &format!("selected: {sel:?}"));

    // End task with Delete (SIGTERM, no confirmation for an ordinary app).
    h.key_press(Key::Delete);
    h.run_steps(3);
    let toast = app(&mut h).toast.as_ref().map(|t| t.text.clone());
    w.step(&mut h, "end-task-delete", &format!("Delete pressed; toast: {toast:?} (pids are fake, so the result is 'already gone' and no signal is sent)"));

    // Force kill with the button, which asks first.
    h.get_by_label("Force kill").click();
    h.run_steps(4);
    assert!(app(&mut h).pending_kill.is_some());
    w.step(&mut h, "force-kill-confirm", "Force kill button pressed: the confirm dialog with Cancel focused");

    // Confirm: the dialog's own button is the last one with that label.
    let buttons: Vec<_> = h.query_all_by_role_and_label(egui::accesskit::Role::Button, "Force kill").collect();
    buttons.last().unwrap().click();
    h.run_steps(3);
    let toast = app(&mut h).toast.as_ref().map(|t| t.text.clone());
    assert!(app(&mut h).pending_kill.is_none());
    w.step(&mut h, "force-kill-done", &format!("confirmed; toast: {toast:?} (fake pids: nothing was signalled)"));
    w.finish();
}

/// Reading focus: the id `--focus <id>` would carry from a temperature alert.
const HOT: &str = "cpu/temp/tctl";

#[test]
#[ignore]
fn walk_t2_after_a_temperature_alert() {
    let _g = layout_lock();
    // The demo clock at 1561 s into the scene: the CPU is throttling and Tctl is past its warning.
    let (snap, hist) = demo_scene(hwglance::demo::T_START + 1561.0);
    let tctl = snap.sensors.iter().flat_map(|g| g.sections.iter()).flat_map(|s| s.readings.iter()).find(|r| r.id == HOT).cloned().unwrap();
    let dir = tmp("ev-t2");
    let mock = dir.join("snapshot.json");
    std::fs::write(&mock, serde_json::to_vec(&snap).unwrap()).unwrap();
    hwglance::settings::save_atomic(&dir.join("ui.json"), &dark(Settings::default()), false).unwrap();
    let opts = hwglance::app::Opts { settings_path: Some(dir.join("ui.json")), no_watchers: true, focus: Some(HOT.to_string()), ..Default::default() };
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(REAL.0, REAL.1)).wgpu().build_ui_state(
        move |ui, app: &mut Option<hwglance::app::App>| {
            if app.is_none() {
                *app = Some(hwglance::app::App::with_ctx(ui.ctx(), None, hwglance::app::Mode::Mock(mock.clone()), opts.clone()));
                return;
            }
            app.as_mut().unwrap().show(ui);
        },
        None,
    );
    let ctx = h.ctx.clone();
    app(&mut h).link.publish_history(&ctx, hist);
    let mut w = Walk::new("T2");
    h.run_steps(8);
    h.hover_at(pos2(1500.0, 3.0));
    h.run_steps(2);
    let consumed = app(&mut h).scroll_to_reading.is_none();
    let note = format!(
        "started with --focus {HOT}; value {:?} {:?}, warn {:?}, crit {:?}; the row flashes (about 1.2 s) and its history popover is pinned; scroll_to_reading consumed: {consumed}",
        tctl.value, tctl.unit, tctl.warn, tctl.crit
    );
    w.step(&mut h, "opened-with-focus", &note);
    let hit = app(&mut h).pinned_hit.clone().expect("row on screen");
    let _ = writeln!(w.log, "row rect {:?}; popover pinned for {:?}", hit.rect, app(&mut h).pinned);
    std::thread::sleep(std::time::Duration::from_millis(700));
    h.run_steps(2);
    w.step(&mut h, "flash-fading", "0.7 s later: the flash is fading, the popover stays pinned");
    std::thread::sleep(std::time::Duration::from_millis(700));
    h.run_steps(2);
    assert!(app(&mut h).flash.is_none());
    w.step(&mut h, "flash-over", "1.4 s later: the flash is over, the popover stays pinned until Esc or a click elsewhere");
    h.key_press(Key::Escape);
    h.run_steps(3);
    assert!(app(&mut h).pinned.is_none());
    w.step(&mut h, "unpinned-with-esc", "Esc lets go of the pinned popover");
    // Hover the row for the popover delay: the same popover, not pinned.
    h.hover_at(hit.rect.center());
    h.run_steps(3);
    std::thread::sleep(std::time::Duration::from_millis(450));
    h.run_steps(4);
    assert!(app(&mut h).hover_pop.is_some());
    w.step(&mut h, "hover-for-history", "pointer on the row for 450 ms: the history popover opens on hover");
    w.finish();
}

#[test]
#[ignore]
fn walk_t3_glance_while_gaming() {
    let _g = layout_lock();
    let size = (2386.0, 1302.0);
    let mut w = Walk::new("T3");
    let mut facts = String::new();
    for (step, at, note) in [
        ("quiet-moment", 30.0, "right compact dock, the four 496 px columns, demo clock at 30 s (nothing throttling)"),
        ("cpu-hot", 1561.0, "same layout while the CPU is hot and throttling (demo clock 1561 s, Tctl past its warning)"),
    ] {
        let (snap, _) = demo_scene(hwglance::demo::T_START + at);
        let s = dark(users_arrangement(Dock::Right, "compact", None));
        let mut h = harness("ev-t3", size, snap, Some(s), true, false);
        h.run_steps(4);
        park(&mut h);
        let a = app(&mut h);
        let panel = a.procs_list_rect.width() + 24.0;
        let mut lefts: Vec<f32> = a.card_rects.iter().map(|(_, r)| (r.left() / 4.0).round() * 4.0).collect();
        lefts.dedup();
        let _ = writeln!(facts, "{step}: process panel about {panel:.0} px wide; sensor card lefts {lefts:?}");
        for (id, r) in &a.card_rects {
            let _ = writeln!(facts, "  card {id}: x {:.0}..{:.0}, y {:.0}..{:.0} (window height {})", r.left(), r.right(), r.top(), r.bottom(), size.1);
        }
        w.step(&mut h, step, note);
        if at > 100.0 {
            // hover the flagged CPU temperature row
            let cpu = a_card(&mut h, "cpu");
            h.hover_at(first_row_pos(cpu));
            h.run_steps(3);
            std::thread::sleep(std::time::Duration::from_millis(450));
            h.run_steps(4);
            w.step(&mut h, "hover-a-row", "pointer on the first CPU row: its history popover");
        }
    }
    write_text("walk/T3", "layout-facts.txt", &facts);
    w.finish();
}

fn a_card(h: &mut H, id: &str) -> egui::Rect {
    app(h).card_rects.iter().find(|(c, _)| c == id).unwrap().1
}

#[test]
#[ignore]
fn walk_t4_customise() {
    let _g = layout_lock();
    let (snap, _) = demo_scene(hwglance::demo::T_START + 30.0);
    let mut h = harness("ev-t4", REAL, snap, Some(dark(Settings::default())), true, false);
    let mut w = Walk::new("T4");
    park(&mut h);
    w.step(&mut h, "start", "default view; Tctl and Tccd1 are the first CPU rows");
    let cpu = a_card(&mut h, "cpu");
    let tctl = first_row_pos(cpu);
    let tccd = tctl + egui::vec2(0.0, 28.0);
    click_secondary(&mut h, tctl);
    w.step(&mut h, "right-click-tctl", "value menu: Watch, Show history, Hide, Show when collapsed, Copy value, Thresholds");
    // threshold fields are drag values; click one to type into it
    let spins: Vec<egui::Rect> = h.query_all_by_role(egui::accesskit::Role::SpinButton).map(|n| n.rect()).collect();
    let _ = writeln!(w.log, "drag value fields in the menu: {spins:?}");
    drag_spin(&mut h, 0, -60.0);
    let (open, thr) = (h.query_by_label("Watch").is_some(), app(&mut h).link.take_demo_limits().last().cloned());
    w.step(&mut h, "warning-dragged", &format!("Warning dragged from 80 C (a click on the field would close the menu); menu still open: {open}; override {thr:?}"));
    drag_spin(&mut h, 1, -28.0);
    let (open, thr) = (h.query_by_label("Watch").is_some(), app(&mut h).link.take_demo_limits().last().cloned());
    w.step(&mut h, "critical-dragged", &format!("Critical dragged from 89 C; menu still open: {open}; override {thr:?}"));
    h.get_by_label("Watch").click();
    h.run_steps(3);
    park(&mut h);
    let a = app(&mut h);
    assert!(a.s.sensors.highlighted.contains(&HOT.to_string()));
    let t = a.link.take_demo_limits().last().cloned();
    let _ = writeln!(w.log, "last limit request for {HOT}: {t:?}");
    w.step(&mut h, "watched", "Watch chosen: the row is highlighted green and carries the new thresholds");
    click_secondary(&mut h, tccd);
    w.step(&mut h, "right-click-tccd1", "value menu on the second row");
    h.get_by_label("Hide").click();
    h.run_steps(3);
    park(&mut h);
    assert!(app(&mut h).s.sensors.hidden.contains(&"cpu/temp/ccd1".to_string()));
    w.step(&mut h, "hidden", "Hide chosen: the row is gone from the card");
    // Settings: the gear button, named by its purpose.
    h.get_by_label("Settings").click();
    h.run_steps(4);
    w.step(&mut h, "settings-opened", "gear clicked: Settings opens on the General tab");
    h.get_by_label("Sensors").click();
    h.run_steps(4);
    w.step(&mut h, "settings-sensors", "Sensors tab: the hidden row is listed with Show, then the watched value and the thresholds");
    h.get_by_label("Show").click();
    h.run_steps(4);
    assert!(app(&mut h).s.sensors.hidden.is_empty());
    w.step(&mut h, "shown-again", "Show clicked: the row is back in the CPU card");
    w.finish();
}

/// Drag the `n`-th drag value of the open menu sideways by `dx` px (a click on it would close
/// the menu: the context menu closes on any click inside).
fn drag_spin(h: &mut H, n: usize, dx: f32) {
    let rect = h.query_all_by_role(egui::accesskit::Role::SpinButton).nth(n).expect("drag value").rect();
    let from = rect.center();
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

// ---------------------------------------------------------------------------------------------
// Measured checks: hit targets, keyboard focus order
// ---------------------------------------------------------------------------------------------

/// The accesskit nodes of a harness that can be hit or focused: everything but text, containers
/// and the window frame. Each line is tab separated: state, role, label, value, x, y, w, h, disabled.
fn dump_targets(h: &H, state: &str, out: &mut String) {
    use egui::accesskit::Role;
    for node in h.root().children_recursive() {
        let ak = node.accesskit_node();
        if matches!(ak.role(), Role::GenericContainer | Role::Label | Role::TextRun | Role::Window | Role::Unknown) && ak.role() != Role::Unknown {
            continue;
        }
        let Some(_) = ak.bounding_box() else { continue };
        let r = node.rect();
        let _ = writeln!(
            out,
            "{state}\t{:?}\t{}\t{}\t{:.1}\t{:.1}\t{:.1}\t{:.1}\t{}",
            ak.role(),
            ak.label().unwrap_or_default().replace('\t', " "),
            ak.value().unwrap_or_default().replace('\t', " "),
            r.min.x,
            r.min.y,
            r.width(),
            r.height(),
            ak.is_disabled()
        );
    }
}

#[test]
#[ignore]
fn evidence_targets() {
    let _g = layout_lock();
    let mut out = String::from("state\trole\tlabel\tvalue\tx\ty\tw\th\tdisabled\n");
    for state in state_names() {
        if matches!(state, "collector-not-running") {
            continue;
        }
        let h = build(state, "dark", REAL);
        dump_targets(&h, state, &mut out);
    }
    // the right dock with the four-column arrangement, in the layout editor (grips and column handles)
    let (snap, _) = demo_scene(hwglance::demo::T_START + 30.0);
    let s = dark(users_arrangement(Dock::Right, "expanded", Some(0.474)));
    let mut h = harness("ev-tg", (2386.0, 1302.0), snap, Some(s), true, false);
    app(&mut h).edit_layout = true;
    h.run_steps(4);
    dump_targets(&h, "edit-layout-right-expanded", &mut out);
    let h = build("collector-not-running", "dark", (1400.0, 600.0));
    dump_targets(&h, "collector-not-running", &mut out);
    write_text("gates/data", "targets-nodes.tsv", &out);
}

/// The focused node of the tree, if any: (role, label, rect).
fn focused(h: &H) -> Option<(String, String, egui::Rect)> {
    h.root().children_recursive().find(|n| n.accesskit_node().is_focused()).map(|n| {
        let ak = n.accesskit_node();
        (format!("{:?}", ak.role()), ak.label().unwrap_or_default(), n.rect())
    })
}

/// Press Tab (or Shift+Tab) up to `max` times and record each stop.
fn tab_walk(h: &mut H, shift: bool, max: usize, out: &mut String, label: &str) -> usize {
    let mut stops = 0;
    let mut first: Option<String> = None;
    for i in 1..=max {
        if shift {
            h.key_press_modifiers(egui::Modifiers::SHIFT, Key::Tab);
        } else {
            h.key_press(Key::Tab);
        }
        h.run_steps(2);
        match focused(h) {
            Some((role, name, r)) => {
                let line = format!("{role}\t{name}\t{:.0},{:.0} {:.0}x{:.0}", r.min.x, r.min.y, r.width(), r.height());
                if first.as_deref() == Some(line.as_str()) {
                    let _ = writeln!(out, "{label}\t{i}\twraps to stop 1");
                    return stops;
                }
                if first.is_none() {
                    first = Some(line.clone());
                }
                stops = i;
                let _ = writeln!(out, "{label}\t{i}\t{line}");
            }
            None => {
                let _ = writeln!(out, "{label}\t{i}\t(nothing focused)");
            }
        }
    }
    stops
}

/// The parts of the state a key could change, to see what Enter did.
fn fingerprint(h: &mut H) -> Vec<(&'static str, String)> {
    let a = app(h);
    vec![
        ("settings json", serde_json::to_string(&a.s).unwrap()),
        ("selection", format!("{:?}", a.selection)),
        ("pinned popover", format!("{:?}", a.pinned)),
        ("expanded groups", format!("{}", a.expanded.len())),
        ("edit layout", format!("{}", a.edit_layout)),
        ("settings window", format!("{}", a.settings_open)),
        ("kill dialog", format!("{}", a.pending_kill.is_some())),
        ("filter", a.filter.clone()),
        ("flash", format!("{}", a.flash.is_some())),
        ("toast", format!("{}", a.toast.is_some())),
    ]
}

#[test]
#[ignore]
fn evidence_keyboard() {
    let _g = layout_lock();
    let mut out = String::from("walk\tstop\trole\tlabel\tposition and size\n");
    // main window, default layout
    let (snap, _) = demo_scene(hwglance::demo::T_START + 30.0);
    let mut h = harness("ev-kb", REAL, snap.clone(), Some(dark(Settings::default())), true, false);
    let fwd = tab_walk(&mut h, false, 900, &mut out, "main Tab");
    let mut h2 = harness("ev-kb2", REAL, snap.clone(), Some(dark(Settings::default())), true, false);
    let back = tab_walk(&mut h2, true, 40, &mut out, "main Shift+Tab");
    // settings window open on General
    let mut s = harness("ev-kb3", REAL, snap.clone(), Some(dark(Settings::default())), true, true);
    let sfwd = tab_walk(&mut s, false, 400, &mut out, "settings Tab");
    let mut s2 = harness("ev-kb3b", REAL, snap.clone(), Some(dark(Settings::default())), true, true);
    tab_walk(&mut s2, true, 40, &mut out, "settings Shift+Tab");
    let mut summary = format!("main window: {fwd} Tab stops recorded before the cap or the wrap; Shift+Tab recorded {back}; settings window: {sfwd}\n");
    // Does Enter or Space on a focused stop do its click? Try the first 24 stops of the main window.
    let _ = writeln!(summary, "activation of stops 1 to 24 and the process panel stops with Enter / Space (what changed):");
    for n in (1..=24).chain([119, 120, 121, 122, 123, 124, 125, 126, 127, 128, 134, 135]) {
        for key in [Key::Enter, Key::Space] {
            let mut h = harness("ev-kb4", REAL, snap.clone(), Some(dark(Settings::default())), false, false);
            for _ in 0..n {
                h.key_press(Key::Tab);
                h.run_steps(2);
            }
            let stop = focused(&h).map(|(r, l, rect)| format!("{r} {l:?} {:.0}x{:.0}", rect.width(), rect.height()));
            let before = fingerprint(&mut h);
            h.key_press(key);
            h.run_steps(3);
            let after = fingerprint(&mut h);
            let changed: Vec<&str> = before.iter().zip(after.iter()).filter(|(b, a)| b.1 != a.1).map(|(b, _)| b.0).collect();
            let _ = writeln!(summary, "stop {n} {stop:?} {key:?}: changed {changed:?}");
        }
    }
    // Context menu from the keyboard: Menu key and Shift+F10 on a focused sensor row.
    let mut h = harness("ev-kb5", REAL, snap, Some(dark(Settings::default())), false, false);
    for _ in 0..12 {
        h.key_press(Key::Tab);
        h.run_steps(2);
    }
    let on = focused(&h).map(|(r, l, _)| format!("{r} {l:?}"));
    // egui has no Menu key: Shift+F10 is the only chord to try
    h.key_press_modifiers(egui::Modifiers::SHIFT, Key::F10);
    h.run_steps(3);
    let shift_f10 = h.query_by_label("Watch").is_some();
    let _ = writeln!(summary, "context menu from the keyboard on {on:?}: Shift+F10 opens it: {shift_f10}");
    write_text("gates/data", "keyboard-tab-order.tsv", &out);
    write_text("gates/data", "keyboard-summary.txt", &summary);
}

// ---------------------------------------------------------------------------------------------
// Contrast: every text-on-tint pair the painters use, in both palettes
// ---------------------------------------------------------------------------------------------

mod contrast {
    use egui::Color32;
    use hwglance::theme::Palette;

    pub fn lum(c: Color32) -> f64 {
        let f = |v: u8| {
            let v = v as f64 / 255.0;
            if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * f(c.r()) + 0.7152 * f(c.g()) + 0.0722 * f(c.b())
    }

    pub fn ratio(a: Color32, b: Color32) -> f64 {
        let (la, lb) = (lum(a), lum(b));
        let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
        (hi + 0.05) / (lo + 0.05)
    }

    /// `fg` at `alpha` painted over `bg`, the way egui composites a `gamma_multiply` tint.
    pub fn wash(bg: Color32, fg: Color32, alpha: f32) -> Color32 {
        bg.blend(fg.gamma_multiply(alpha))
    }

    pub fn hex(c: Color32) -> String {
        format!("#{:02X}{:02X}{:02X}", c.r(), c.g(), c.b())
    }

    pub struct Pair {
        pub what: String,
        pub fg: Color32,
        pub bg: Color32,
        /// What the element needs: 4.5 for text, 3.0 for non-text marks.
        pub need: f64,
    }

    pub fn pairs(p: &Palette) -> Vec<Pair> {
        let mut v = Vec::new();
        let mut add = |what: String, fg: Color32, bg: Color32, need: f64| v.push(Pair { what, fg, bg, need });
        let surfaces = [("ground", p.ground), ("panel", p.panel), ("header", p.header), ("alt", p.alt), ("zebra", p.zebra), ("hover", p.hover), ("selected", p.selected)];
        let inks = [
            ("ink", p.ink),
            ("ink2", p.ink2),
            ("ink3", p.ink3),
            ("cpu", p.cpu),
            ("gpu", p.gpu),
            ("mem", p.mem),
            ("drive", p.drive),
            ("board", p.board),
            ("igpu", p.igpu),
            ("warn", p.warn),
            ("crit", p.crit),
            ("watch", p.watch),
        ];
        let devices = [("cpu", p.cpu), ("gpu", p.gpu), ("mem", p.mem), ("drive", p.drive), ("board", p.board), ("igpu", p.igpu)];
        for (sn, s) in surfaces {
            for (iname, i) in inks {
                add(format!("{iname} text on {sn}"), i, s, 4.5);
            }
        }
        add("on_crit on crit (Throttling now pill)".into(), p.on_crit, p.crit, 4.5);
        // the app badge without an icon: ink 3 over the row at each strength, the letter in ink
        for (rn, row) in [("panel", p.panel), ("zebra", p.zebra), ("hover", p.hover), ("selected", p.selected)] {
            for a in hwglance::procs::badge_tint_strengths() {
                add(format!("app badge letter (ink on {:.0} % ink 3) over {rn}", a * 100.0), p.ink, wash(row, p.ink3, a), 4.5);
            }
        }
        // radio, checkbox, button and slider-rail outlines on the surfaces controls sit on
        for (sn, s) in [("ground", p.ground), ("panel", p.panel), ("header", p.header), ("hover", p.hover)] {
            add(format!("control outline and slider rail on {sn} (non-text)"), p.control(), s, 3.0);
        }
        let (fill, ink) = p.crit_button();
        add("kill button text on its fill".into(), ink, fill, 4.5);
        for (dn, d) in devices {
            add(format!("ground text on {dn} device tab (dark window)"), p.ground, d, 4.5);
            for (rn, row) in [("panel", p.panel), ("zebra", p.zebra), ("hover", p.hover), ("selected", p.selected)] {
                let cell = wash(row, d, 0.16);
                add(format!("ink on heavy {dn} cell (16 %) over {rn}"), p.ink, cell, 4.5);
                add(format!("ink2 on heavy {dn} cell (16 %) over {rn}"), p.ink2, cell, 4.5);
            }
            for (bn, bg) in [("header", p.header), ("hover", p.hover)] {
                let fill = hwglance::ui_kit::tab_fill(d, bg);
                add(format!("device tab {dn} text on its tint over {bn}"), hwglance::ui_kit::tab_ink(d, fill), fill, 4.5);
            }
            // process table header: the device colour at 90 % over the header band
            add(format!("process column header {dn} (95 % alpha) on header"), p.header.blend(d.gamma_multiply(hwglance::procs::HEADER_REST_ALPHA)), p.header, 4.5);
            let track = hwglance::rangebar::track_color(d, p);
            let range = wash(track, d, p.range_alpha);
            add(format!("range bar {dn}: observed range vs track (non-text)"), range, track, 3.0);
            for (mn, m) in [("ink", p.ink), ("warn", p.warn), ("crit", p.crit), ("ink3", p.ink3)] {
                add(format!("range bar {dn}: {mn} marker on track (non-text)"), m, track, 3.0);
            }
        }
        for (rn, row) in [("panel", p.panel), ("alt", p.alt), ("hover", p.hover)] {
            let watched = wash(row, p.watch, hwglance::sensors::WATCH_WASH);
            add(format!("watch text on watched row (9 %) over {rn}"), p.watch, watched, 4.5);
            add(format!("warn on watched row over {rn}"), p.warn, watched, 4.5);
            add(format!("crit on watched row over {rn}"), p.crit, watched, 4.5);
            add(format!("ink on watched row over {rn}"), p.ink, watched, 4.5);
        }
        for a in [hwglance::sensors::FLASH_PEAK, 0.3, 0.15] {
            add(format!("ink on the focus flash at {:.0} % green", a * 100.0), p.ink, wash(p.panel, p.watch, a), 4.5);
        }
        add("ink on the kill warning box (18 % red)".into(), p.ink, wash(p.panel, p.crit, 0.18), 4.5);
        let (bfill, reason) = p.crit_banner();
        let banner = p.ground.blend(bfill);
        add("crit heading on the collector-down banner".into(), p.crit, banner, 4.5);
        add("reason text on the collector-down banner".into(), reason, banner, 4.5);
        add("ink2 on the history plot's red band (45 %) (non-text)".into(), p.ink2, wash(p.panel, p.crit, 0.45), 3.0);
        add("rule stroke on panel (non-text, 1.3 is the test's own floor)".into(), p.rule, p.panel, 1.3);
        v
    }
}

/// Every text pair the painters use reaches 4.5:1 and every outline 3:1, in both palettes.
#[test]
fn no_painted_pair_is_under_its_target() {
    use contrast::*;
    let mut under = Vec::new();
    for (name, p) in [("dark", Palette::dark()), ("light", Palette::light())] {
        for x in pairs(&p).iter().filter(|x| ratio(x.fg, x.bg) < x.need) {
            under.push(format!("{name}: {} {} on {}: {:.2} (needs {})", x.what, hex(x.fg), hex(x.bg), ratio(x.fg, x.bg), x.need));
        }
    }
    assert!(under.is_empty(), "pairs under their target:\n{}", under.join("\n"));
}

#[test]
#[ignore]
fn evidence_contrast() {
    use contrast::*;
    let mut md = String::new();
    for (name, p) in [("dark", Palette::dark()), ("light", Palette::light())] {
        let all = pairs(&p);
        let _ = writeln!(md, "## {name} palette: {} pairs checked\n", all.len());
        let under: Vec<&Pair> = all.iter().filter(|x| ratio(x.fg, x.bg) < x.need).collect();
        let _ = writeln!(md, "Pairs under their target ({} of {}):\n", under.len(), all.len());
        let _ = writeln!(md, "| pair | foreground | background | ratio | target |\n|---|---|---|---|---|");
        let mut under = under;
        under.sort_by(|a, b| ratio(a.fg, a.bg).total_cmp(&ratio(b.fg, b.bg)));
        for x in under {
            let _ = writeln!(md, "| {} | {} | {} | {:.2} | {:.1} |", x.what, hex(x.fg), hex(x.bg), ratio(x.fg, x.bg), x.need);
        }
        let _ = writeln!(md);
        let min_text = all.iter().filter(|x| x.need == 4.5).map(|x| ratio(x.fg, x.bg)).fold(f64::MAX, f64::min);
        let _ = writeln!(md, "Lowest text ratio in this palette: {min_text:.2}\n");
        let _ = writeln!(md, "<details><summary>all pairs</summary>\n\n| pair | fg | bg | ratio |\n|---|---|---|---|");
        for x in &all {
            let _ = writeln!(md, "| {} | {} | {} | {:.2} |", x.what, hex(x.fg), hex(x.bg), ratio(x.fg, x.bg));
        }
        let _ = writeln!(md, "\n</details>\n");
    }
    write_text("gates/data", "contrast-pairs.md", &md);
}

use hwglance::theme::Palette;

// ---------------------------------------------------------------------------------------------
// Performance: frame cost per snapshot and what the app asks the event loop for
// ---------------------------------------------------------------------------------------------

fn repaint_delay(h: &H) -> std::time::Duration {
    h.output().viewport_output.get(&egui::ViewportId::ROOT).map(|v| v.repaint_delay).expect("root viewport")
}

fn delay_text(d: std::time::Duration) -> String {
    if d == std::time::Duration::MAX || d.as_secs() > 1_000_000 {
        "none (sleeps until input or new data)".into()
    } else {
        format!("{:.3} s", d.as_secs_f64())
    }
}

/// Run with `cargo test --release -p hwglance --test evidence -- --ignored evidence_perf`.
#[test]
#[ignore]
fn evidence_perf() {
    let _g = layout_lock();
    let mut md = String::new();
    let scenes: Vec<(&str, (f32, f32), Settings)> = vec![
        ("bottom dock, default layout, 2546x1400", REAL, dark(Settings::default())),
        ("right expanded dock, four columns, 2386x1302", (2386.0, 1302.0), dark(users_arrangement(Dock::Right, "expanded", Some(0.474)))),
        ("right compact dock, four columns, 2386x1302", (2386.0, 1302.0), dark(users_arrangement(Dock::Right, "compact", None))),
        ("bottom dock, half width 1269x1400", (1269.0, 1400.0), dark(Settings::default())),
    ];
    let _ = writeln!(md, "| scene | frames | median ms | mean ms | p95 ms | max ms | % of one core at 1 snapshot/s (mean) |\n|---|---|---|---|---|---|---|");
    for (name, size, settings) in scenes {
        let mut demo = hwglance::demo::Demo::new();
        let mut h = harness("ev-perf", size, demo.snapshot(1.0), Some(settings), false, false);
        h.hover_at(pos2(size.0 - 40.0, 3.0));
        h.run_steps(3);
        let mut times = Vec::new();
        for i in 0..240 {
            let ctx = h.ctx.clone();
            let snap = demo.snapshot(2.0 + i as f64);
            app(&mut h).link.publish(&ctx, snap);
            let t = std::time::Instant::now();
            h.step();
            let shapes = h.output().shapes.clone();
            let _ = h.ctx.tessellate(shapes, 1.0);
            times.push(t.elapsed().as_secs_f64() * 1000.0);
        }
        times.sort_by(|a, b| a.total_cmp(b));
        let mean = times.iter().sum::<f64>() / times.len() as f64;
        let n = times.len();
        let _ = writeln!(md, "| {name} | {n} | {:.2} | {mean:.2} | {:.2} | {:.2} | {:.2} % |", times[n / 2], times[n * 95 / 100], times[n - 1], mean / 10.0);
    }

    // What the window asks the event loop for after a frame, in each situation. Each row runs
    // eight frames with no new input first, so animations and one-off repaints have settled.
    let _ = writeln!(md, "\n| situation | repaint requested after the settled frame |\n|---|---|");
    let (snap, hist) = demo_scene(hwglance::demo::T_START + 30.0);
    let mut h = harness("ev-perf2", REAL, snap.clone(), Some(dark(Settings::default())), false, false);
    h.hover_at(pos2(REAL.0 - 40.0, 3.0));
    h.run_steps(4);
    let ctx = h.ctx.clone();
    // mock sources are never stale; a live link is, so measure it as a live link would behave
    app(&mut h).link.shared.lock().unwrap().never_stale = false;
    let mut demo = hwglance::demo::Demo::new();
    app(&mut h).link.publish(&ctx, demo.snapshot(5.0));
    h.step();
    let _ = writeln!(md, "| the frame that shows a new 1 s snapshot (publish asks for an immediate repaint) | {} |", delay_text(repaint_delay(&h)));
    h.run_steps(8);
    let _ = writeln!(md, "| idle: pointer parked, snapshot shown, no input | {} |", delay_text(repaint_delay(&h)));
    let cpu = a_card(&mut h, "cpu");
    h.hover_at(first_row_pos(cpu));
    h.step();
    let _ = writeln!(md, "| pointer just arrived on a sensor row | {} |", delay_text(repaint_delay(&h)));
    h.run_steps(1);
    let _ = writeln!(md, "| pointer resting on the row, before the popover delay | {} |", delay_text(repaint_delay(&h)));
    std::thread::sleep(std::time::Duration::from_millis(450));
    h.run_steps(8);
    let open = app(&mut h).hover_pop.is_some();
    let _ = writeln!(md, "| history popover open on hover (open: {open}), settled | {} |", delay_text(repaint_delay(&h)));
    h.hover_at(pos2(REAL.0 - 40.0, 3.0));
    h.run_steps(8);
    app(&mut h).link.publish_history(&ctx, hist);
    app(&mut h).apply_focus(&ctx, HOT, false);
    h.step();
    let _ = writeln!(md, "| focus flash running (about 1.2 s of wall time) | {} |", delay_text(repaint_delay(&h)));
    std::thread::sleep(std::time::Duration::from_millis(1400));
    h.run_steps(8);
    let _ = writeln!(md, "| popover pinned, flash over, settled | {} |", delay_text(repaint_delay(&h)));
    app(&mut h).pinned = None;
    h.run_steps(8);
    app(&mut h).set_toast("test".into(), true);
    h.run_steps(8);
    let _ = writeln!(md, "| a toast showing, settled | {} |", delay_text(repaint_delay(&h)));
    // a stale link: no data for more than three intervals
    let mut h = harness("ev-perf3", REAL, snap, Some(dark(Settings::default())), false, false);
    h.hover_at(pos2(REAL.0 - 40.0, 3.0));
    h.run_steps(3);
    app(&mut h).link.shared.lock().unwrap().never_stale = false;
    app(&mut h).link.shared.lock().unwrap().received = Some(std::time::Instant::now());
    std::thread::sleep(std::time::Duration::from_millis(3200));
    h.run_steps(8);
    let _ = writeln!(md, "| data stale (no snapshot for more than 3 intervals; the age readout moves), settled | {} |", delay_text(repaint_delay(&h)));
    write_text("gates/data", "perf-window.md", &md);
    println!("{md}");
}

/// Cost of decoding one live snapshot line (the collector's JSON), on the window's reader thread.
/// `HWGLANCE_LIVE_LINE=<file with one snapshot line>`; run in release.
#[test]
#[ignore]
fn evidence_parse_cost() {
    let Some(p) = std::env::var_os("HWGLANCE_LIVE_LINE") else { return };
    let line = std::fs::read_to_string(p).unwrap();
    let mut times = Vec::new();
    for _ in 0..200 {
        let t = std::time::Instant::now();
        let msg: hwglance_proto::DaemonMsg = serde_json::from_str(line.trim_end()).unwrap();
        times.push(t.elapsed().as_secs_f64() * 1000.0);
        std::hint::black_box(msg);
    }
    times.sort_by(|a, b| a.total_cmp(b));
    let text = format!("one live snapshot line: {} bytes; decode median {:.2} ms, p95 {:.2} ms, max {:.2} ms over 200 runs\n", line.len(), times[100], times[190], times[199]);
    println!("{text}");
    write_text("gates/data", "perf-parse.txt", &text);
}

/// What a screen reader would get: node counts by kind per state, and whether the text of the
/// readings and the process rows is reachable at all.
#[test]
#[ignore]
fn evidence_a11y() {
    use egui::accesskit::Role;
    let _g = layout_lock();
    let mut out = String::from("state\tnodes\tlabels(text)\tnamed buttons/controls\tunnamed interactive (role Unknown)\tother\n");
    let mut probes = String::new();
    for state in state_names() {
        if state == "collector-not-running" {
            continue;
        }
        let h = build(state, "dark", REAL);
        let (mut nodes, mut labels, mut named, mut unnamed, mut other) = (0, 0, 0, 0, 0);
        for node in h.root().children_recursive() {
            let ak = node.accesskit_node();
            nodes += 1;
            match ak.role() {
                Role::Label | Role::TextRun => labels += 1,
                Role::GenericContainer | Role::Window => other += 1,
                Role::Unknown => unnamed += 1,
                _ => {
                    if ak.label().is_some() || ak.value().is_some() {
                        named += 1
                    } else {
                        unnamed += 1
                    }
                }
            }
        }
        let _ = writeln!(out, "{state}\t{nodes}\t{labels}\t{named}\t{unnamed}\t{other}");
        if state == "default" || state == "processes-expanded" {
            for probe in ["Tctl", "GPU Hotspot", "Elden Ring", "Brave", "CPU Package Power", "Thermal throttling", "84.", "Throttled", "Sources 4/4"] {
                let _ = writeln!(probes, "{state}: text containing {probe:?} reachable by a screen reader: {}", h.query_all_by_label_contains(probe).next().is_some());
            }
        }
    }
    write_text("gates/data", "a11y-counts.tsv", &out);
    write_text("gates/data", "a11y-probes.txt", &probes);
}

/// Which nodes `evidence_a11y` still counts as unnamed, per state (printed, nothing written).
#[test]
#[ignore]
fn evidence_a11y_which_unnamed() {
    use egui::accesskit::Role;
    let _g = layout_lock();
    for state in ["default", "history-popover", "value-menu", "processes-expanded"] {
        let h = build(state, "dark", REAL);
        for node in h.root().children_recursive() {
            let ak = node.accesskit_node();
            let unnamed = match ak.role() {
                Role::Label | Role::TextRun | Role::GenericContainer | Role::Window => false,
                Role::Unknown => true,
                _ => ak.label().is_none() && ak.value().is_none(),
            };
            if unnamed {
                let r = node.rect();
                println!("UNNAMED {state} {:?} {:.0}x{:.0} at {:.0},{:.0}", ak.role(), r.width(), r.height(), r.min.x, r.min.y);
            }
        }
    }
}

/// What building the accessibility tree costs per frame, in this debug build: the same scene
/// run through a bare egui context (no harness, which insists on the tree) with the tree on (what
/// a connected screen reader gets) and off (no assistive technology, which is how the shipped
/// window runs until one asks).
#[test]
#[ignore]
fn evidence_accesskit_cost() {
    use hwglance::app::{App, Mode, Opts};
    let _g = layout_lock();
    let mut demo = hwglance::demo::Demo::new();
    let dir = tmp("ak-cost");
    let mock = dir.join("snapshot.json");
    std::fs::write(&mock, serde_json::to_vec(&demo.snapshot(1.0)).unwrap()).unwrap();
    let ctx = egui::Context::default();
    let opts = Opts { settings_path: Some(dir.join("ui.json")), no_watchers: true, ..Default::default() };
    let mut app = App::with_ctx(&ctx, None, Mode::Mock(mock.clone()), opts);
    let mut now = 0.0;
    let mut frame = |ctx: &egui::Context, app: &mut App| {
        now += 1.0;
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(pos2(0.0, 0.0), egui::vec2(2048.0, 1150.0))),
            time: Some(now),
            ..Default::default()
        };
        let mut out = ctx.run_ui(input, |ui| app.show(ui));
        out.textures_delta.clear();
        let _ = ctx.tessellate(out.shapes, 1.0);
        out.platform_output.accesskit_update.is_some()
    };
    for _ in 0..8 {
        frame(&ctx, &mut app);
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let mut measure = |on: bool| -> (f64, bool) {
        if on {
            ctx.enable_accesskit();
        } else {
            ctx.disable_accesskit();
        }
        let mut times = Vec::new();
        let mut built = false;
        for _ in 0..60 {
            let t = std::time::Instant::now();
            built |= frame(&ctx, &mut app);
            times.push(t.elapsed().as_secs_f64() * 1000.0);
        }
        times.sort_by(|a, b| a.total_cmp(b));
        (times[30], built)
    };
    let (mut off, mut on) = (Vec::new(), Vec::new());
    for _ in 0..3 {
        off.push(measure(false));
        on.push(measure(true));
    }
    let text = format!("median frame ms (debug build, 2048x1150, no GPU): tree off {off:.2?}, tree on {on:.2?} (second field: a tree update was produced)\n");
    println!("{text}");
    write_text("gates/data", "accesskit-cost.txt", &text);
}
