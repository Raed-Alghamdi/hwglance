//! Interaction tests: keyboard navigation, search focus, the kill flow against a dummy process,
//! ui.json persistence and the process list stability.

mod common;

use common::*;
use egui::{pos2, Key};
use egui_kittest::kittest::{NodeT, Queryable};
use hwglance::actions::parse_start_ticks;
use hwglance::model::RowId;
use hwglance::settings::{self, Settings};
use hwglance_proto::{ProcClass, ProcGroup, ProcInfo};
use std::os::unix::process::ExitStatusExt;
use std::time::Duration;

#[test]
fn keyboard_navigation_and_search_focus() {
    let mut h = harness("nav", (1600.0, 900.0), demo_snapshot(), None, false, false);
    h.key_press(Key::ArrowDown);
    h.run_steps(2);
    assert_eq!(app(&mut h).selection, Some(RowId::Section(ProcClass::App)));
    h.key_press(Key::ArrowDown);
    h.run_steps(2);
    assert!(matches!(app(&mut h).selection, Some(RowId::Group(_))));
    h.key_press(Key::ArrowUp);
    h.run_steps(2);
    assert_eq!(app(&mut h).selection, Some(RowId::Section(ProcClass::App)));

    // "/" focuses the search box without typing the slash.
    h.key_press(Key::Slash);
    h.run_steps(3);
    assert!(h.ctx.egui_wants_keyboard_input());
    assert_eq!(app(&mut h).filter, "");
}

#[test]
fn right_expands_and_left_collapses_a_group() {
    let mut h = harness("expand", (1600.0, 900.0), demo_snapshot(), None, false, false);
    let id = {
        let a = app(&mut h);
        a.groups().iter().find(|g| g.class == ProcClass::App && g.procs.len() > 1).unwrap().id.clone()
    };
    app(&mut h).selection = Some(RowId::Group(id.clone()));
    h.key_press(Key::ArrowRight);
    h.run_steps(2);
    assert!(app(&mut h).expanded.contains(&id));
    h.key_press(Key::ArrowLeft);
    h.run_steps(2);
    assert!(!app(&mut h).expanded.contains(&id));
}

#[test]
fn force_kill_requires_confirmation_and_kills_the_dummy() {
    let mut child = std::process::Command::new("sleep").arg("600").spawn().unwrap();
    let pid = child.id() as i32;
    let ticks = parse_start_ticks(&std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap()).unwrap();
    let uid = unsafe { libc::getuid() };
    let mut snap = demo_snapshot();
    let p = ProcInfo {
        pid,
        ppid: 1,
        uid,
        name: "sleep".into(),
        cmdline: "sleep 600".into(),
        state: 'S',
        threads: 1,
        cpu_pct: 0.0,
        mem_bytes: 1 << 20,
        gpu_pct: None,
        gpu_enc_pct: None,
        gpu_dec_pct: None,
        vram_bytes: None,
        start_time_ticks: ticks,
        exe: None,
        disk_read_bps: None,
        disk_write_bps: None,
        started_unix_ms: 0,
        user: String::new(),
        critical: None,
    };
    snap.procs.as_mut().unwrap().push(ProcGroup {
        id: "app:dummy-sleep".into(),
        class: ProcClass::App,
        disk_read_bps: None,
        disk_write_bps: None,
        gpu_enc_pct: None,
        gpu_dec_pct: None,
        name: "Dummy sleep".into(),
        icon: None,
        title: None,
        procs: vec![p],
        cpu_pct: 0.0,
        mem_bytes: 1 << 20,
        gpu_pct: None,
        vram_bytes: None,
    });
    let mut h = harness("kill", (1600.0, 900.0), snap, None, false, false);
    app(&mut h).selection = Some(RowId::Group("app:dummy-sleep".into()));

    h.key_press_modifiers(egui::Modifiers::SHIFT, Key::Delete);
    h.run_steps(3);
    assert!(app(&mut h).pending_kill.is_some());
    assert!(child.try_wait().unwrap().is_none());

    h.key_press(Key::Escape);
    h.run_steps(2);
    assert!(app(&mut h).pending_kill.is_none());
    assert!(child.try_wait().unwrap().is_none());

    // Enter does not fire a force kill: Cancel has the focus when the dialog opens.
    h.key_press_modifiers(egui::Modifiers::SHIFT, Key::Delete);
    h.run_steps(3);
    assert!(app(&mut h).pending_kill.is_some());
    h.key_press(Key::Enter);
    h.run_steps(3);
    assert!(child.try_wait().unwrap().is_none(), "Enter must not send SIGKILL");
    assert!(app(&mut h).pending_kill.is_none(), "Enter pressed Cancel");

    // Tab moves on to Force kill, and Enter on it confirms.
    h.key_press_modifiers(egui::Modifiers::SHIFT, Key::Delete);
    h.run_steps(3);
    assert!(child.try_wait().unwrap().is_none());
    h.key_press(Key::Tab);
    h.run_steps(3);
    assert!(app(&mut h).pending_kill.is_some(), "Tab alone changes nothing");
    h.key_press(Key::Enter);
    h.run_steps(3);
    let status = child.wait().unwrap();
    assert_eq!(status.signal(), Some(libc::SIGKILL));
}

#[test]
fn clicking_force_kill_in_the_dialog_kills() {
    let mut child = std::process::Command::new("sleep").arg("600").spawn().unwrap();
    let pid = child.id() as i32;
    let ticks = parse_start_ticks(&std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap()).unwrap();
    let mut snap = demo_snapshot();
    snap.procs.as_mut().unwrap().push(ProcGroup {
        id: "app:dummy-sleep2".into(),
        class: ProcClass::App,
        disk_read_bps: None,
        disk_write_bps: None,
        gpu_enc_pct: None,
        gpu_dec_pct: None,
        name: "Dummy sleep two".into(),
        icon: None,
        title: None,
        procs: vec![ProcInfo {
            pid,
            ppid: 1,
            uid: unsafe { libc::getuid() },
            name: "sleep".into(),
            cmdline: "sleep 600".into(),
            state: 'S',
            threads: 1,
            cpu_pct: 0.0,
            mem_bytes: 1 << 20,
            gpu_pct: None,
            gpu_enc_pct: None,
            gpu_dec_pct: None,
            vram_bytes: None,
            start_time_ticks: ticks,
            exe: None,
            disk_read_bps: None,
            disk_write_bps: None,
            started_unix_ms: 0,
            user: String::new(),
            critical: None,
        }],
        cpu_pct: 0.0,
        mem_bytes: 1 << 20,
        gpu_pct: None,
        vram_bytes: None,
    });
    let mut h = harness("kill-click", (1600.0, 900.0), snap, None, false, false);
    app(&mut h).selection = Some(RowId::Group("app:dummy-sleep2".into()));
    h.key_press_modifiers(egui::Modifiers::SHIFT, Key::Delete);
    h.run_steps(3);
    assert!(app(&mut h).pending_kill.is_some());
    // two buttons carry the words: the action bar's (behind the dialog) and the dialog's, which is last
    let buttons: Vec<_> = h.query_all_by_role_and_label(egui::accesskit::Role::Button, "Force kill").collect();
    assert!(buttons.len() >= 2, "{}", buttons.len());
    buttons.last().unwrap().click();
    h.run_steps(3);
    let status = child.wait().unwrap();
    assert_eq!(status.signal(), Some(libc::SIGKILL));
}

/// First reading of the CPU card: temperatures lead every card, and Tctl is their first row.
const FIRST_CPU_ROW: &str = "cpu/temp/tctl";

/// A point on the first data row of the CPU card (header, caption row, section head, then the row).
fn first_row_pos(card: egui::Rect) -> egui::Pos2 {
    pos2(card.left() + 100.0, card.top() + 36.0 + 20.0 + 28.0 + 14.0)
}

fn wait_and_step(h: &mut H, ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
    h.run_steps(2);
}

#[test]
fn settings_are_written_debounced_and_atomically() {
    let mut h = harness("persist", (1600.0, 900.0), demo_snapshot(), None, false, false);
    let path = app(&mut h).settings_path.clone();
    assert!(!path.exists(), "a missing file means defaults and nothing is written yet");

    app(&mut h).s.appearance.font_size = 16.0;
    app(&mut h).s.sensors.highlighted.push("cpu/temp/tctl".into());
    h.run_steps(2);
    assert!(!path.exists(), "debounced: not written right away");
    wait_and_step(&mut h, 700);
    h.run_steps(2);
    let written = settings::load(&path);
    assert!(written.existed && !written.corrupt);
    assert_eq!(written.settings.appearance.font_size, 16.0);
    assert_eq!(written.settings.sensors.highlighted, vec!["cpu/temp/tctl"]);
    // only ui.json is left in the directory, no temp files
    let n = std::fs::read_dir(path.parent().unwrap()).unwrap().count();
    assert_eq!(n, 2, "snapshot.json and ui.json");
}

#[test]
fn external_edits_are_picked_up_but_own_writes_are_not_echoed() {
    let mut h = harness("external", (1600.0, 900.0), demo_snapshot(), None, false, false);
    let path = app(&mut h).settings_path.clone();
    app(&mut h).s.processes.resort_seconds = 9.0;
    h.run_steps(2);
    wait_and_step(&mut h, 700);
    h.run_steps(2);
    assert_eq!(settings::load(&path).settings.processes.resort_seconds, 9.0);

    // The watcher fires for our own write: nothing changes.
    app(&mut h).external_change.store(true, std::sync::atomic::Ordering::Relaxed);
    h.run_steps(2);
    assert_eq!(app(&mut h).s.processes.resort_seconds, 9.0);

    // Someone else rewrites the file (the bar widget never does, but a text editor might).
    std::thread::sleep(Duration::from_millis(20));
    let mut other = settings::load(&path).settings;
    other.processes.resort_seconds = 4.0;
    other.sensors.hidden.push("a/b".into());
    std::fs::write(&path, settings::to_json(&other)).unwrap();
    app(&mut h).external_change.store(true, std::sync::atomic::Ordering::Relaxed);
    h.run_steps(2);
    assert_eq!(app(&mut h).s.processes.resort_seconds, 4.0);
    assert_eq!(app(&mut h).s.sensors.hidden, vec!["a/b"]);
}

#[test]
fn first_run_migrates_nothing_and_loads_saved_layout() {
    let mut s = Settings::default();
    s.layout.collapsed_cards.push("gpu0".into());
    s.sensors.hidden_cards.push("igpu".into());
    let mut h = harness("load", (1600.0, 900.0), demo_snapshot(), Some(s), false, false);
    assert!(app(&mut h).s.layout.collapsed_cards.contains(&"gpu0".to_string()));
    assert!(app(&mut h).s.sensors.hidden_cards.contains(&"igpu".to_string()));
    // Loading does not rewrite the file.
    let path = app(&mut h).settings_path.clone();
    let before = std::fs::metadata(&path).unwrap().modified().unwrap();
    wait_and_step(&mut h, 700);
    assert_eq!(std::fs::metadata(&path).unwrap().modified().unwrap(), before);
}

fn publish(h: &mut H, snap: hwglance_proto::Snapshot) {
    let ctx = h.ctx.clone();
    app(h).link.publish(&ctx, snap);
    h.run_steps(2);
}

fn group_order(h: &mut H) -> Vec<String> {
    let a = app(h);
    a.rows
        .iter()
        .filter_map(|r| match &r.id {
            RowId::Group(g) => Some(g.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn rows_stay_put_while_hovered_and_resort_after_the_interval_when_not() {
    let mut s = Settings::default();
    s.processes.smoothing_seconds = 0.0;
    s.processes.resort_seconds = 1.0;
    let base = demo_snapshot();
    let mut h = harness("hold", (1600.0, 900.0), base.clone(), Some(s), false, false);
    // Pointer over the process list.
    let list = app(&mut h).procs_list_rect;
    h.hover_at(list.center());
    h.run_steps(3);
    assert!(app(&mut h).procs_hovered);
    let before = group_order(&mut h);

    // Make the last App group the busiest one and publish: a re-sort would move it to the top.
    let mut changed = base.clone();
    changed.seq += 1;
    changed.unix_ms += 1000;
    {
        let groups = changed.procs.as_mut().unwrap();
        let last = groups.iter_mut().rev().find(|g| g.class == ProcClass::App).unwrap();
        last.cpu_pct = 900.0;
        last.procs[0].cpu_pct = 900.0;
    }
    std::thread::sleep(Duration::from_millis(1100));
    publish(&mut h, changed.clone());
    assert_eq!(group_order(&mut h), before, "values update, rows do not move while hovered");

    // Pointer away: the next snapshot re-sorts because the interval has passed.
    h.hover_at(pos2(800.0, 12.0));
    h.run_steps(3);
    assert!(!app(&mut h).procs_hovered);
    changed.seq += 1;
    changed.unix_ms += 1000;
    publish(&mut h, changed);
    let after = group_order(&mut h);
    assert_ne!(after, before);
    let busiest = app(&mut h).groups().iter().filter(|g| g.class == ProcClass::App).max_by(|a, b| a.cpu_pct.total_cmp(&b.cpu_pct)).unwrap().id.clone();
    assert_eq!(after[0], busiest);
}

#[test]
fn resort_is_throttled_when_not_hovering() {
    let mut s = Settings::default();
    s.processes.smoothing_seconds = 0.0;
    s.processes.resort_seconds = 30.0;
    let base = demo_snapshot();
    let mut h = harness("throttle", (1600.0, 900.0), base.clone(), Some(s), false, false);
    let before = group_order(&mut h);
    let mut changed = base;
    changed.seq += 1;
    changed.unix_ms += 1000;
    {
        let groups = changed.procs.as_mut().unwrap();
        let last = groups.iter_mut().rev().find(|g| g.class == ProcClass::App).unwrap();
        last.cpu_pct = 900.0;
        last.procs[0].cpu_pct = 900.0;
    }
    publish(&mut h, changed);
    assert_eq!(group_order(&mut h), before, "30 s have not passed since the last sort");
}

#[test]
fn smoothing_applies_to_displayed_values() {
    let mut s = Settings::default();
    s.processes.smoothing_seconds = 3.0;
    let base = demo_snapshot();
    let mut h = harness("smooth", (1600.0, 900.0), base.clone(), Some(s), false, false);
    let gid = app(&mut h).groups().iter().find(|g| g.class == ProcClass::App).unwrap().id.clone();
    let raw0 = base.procs.as_ref().unwrap().iter().find(|g| g.id == gid).unwrap().cpu_pct;
    let mut next = base.clone();
    next.seq += 1;
    next.unix_ms += 1000;
    for g in next.procs.as_mut().unwrap().iter_mut().filter(|g| g.id == gid) {
        g.cpu_pct = raw0 + 60.0;
    }
    publish(&mut h, next);
    let shown = app(&mut h).groups().iter().find(|g| g.id == gid).unwrap().cpu_pct;
    let expect = raw0 + 60.0 * (1.0 - (-1.0f32 / 3.0).exp());
    assert!((shown - expect).abs() < 0.05, "shown {shown}, expected {expect}");
}

fn drag(h: &mut H, from: egui::Pos2, to: egui::Pos2) {
    h.event(egui::Event::PointerMoved(from));
    h.run_steps(2);
    h.event(egui::Event::PointerButton { pos: from, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE });
    h.run_steps(2);
    for i in 1..=10 {
        let t = i as f32 / 10.0;
        h.event(egui::Event::PointerMoved(from + (to - from) * t));
        h.run_steps(2);
    }
    h.event(egui::Event::PointerButton { pos: to, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE });
    h.run_steps(4);
}

#[test]
fn dragging_a_card_by_its_grip_freezes_the_layout_and_moves_it() {
    let _guard = layout_lock();
    let mut h = harness("dnd", (2048.0, 1150.0), demo_snapshot(), None, false, false);
    assert!(app(&mut h).s.layout.auto);
    // Without edit mode a drag on the card title moves nothing.
    drag(&mut h, pos2(28.0, 100.0), pos2(1700.0, 110.0));
    assert!(app(&mut h).s.layout.auto, "layout is locked outside edit mode");

    app(&mut h).edit_layout = true;
    h.run_steps(3);
    // The CPU card's grip is at the left of its header band, below the edit bar.
    let cpu = app(&mut h).card_rects.iter().find(|(id, _)| id == "cpu").unwrap().1;
    let last = app(&mut h).card_rects.iter().map(|(_, r)| r.left()).fold(0.0, f32::max);
    drag(&mut h, pos2(cpu.left() + 12.0, cpu.top() + 18.0), pos2(last + 100.0, cpu.top() + 24.0));
    let l = &app(&mut h).s.layout;
    assert!(!l.auto, "the first edit freezes the automatic arrangement");
    let first_col_cards: Vec<&str> = l.columns[0].cards.iter().map(|s| s.as_str()).collect();
    assert!(!first_col_cards.contains(&"cpu"), "cpu left column 1: {:?}", l.columns);
    let last = l.columns.last().unwrap();
    assert_eq!(last.cards.first().map(|s| s.as_str()), Some("cpu"), "{:?}", l.columns);
    // every card is still somewhere exactly once
    let all: Vec<&String> = l.columns.iter().flat_map(|c| c.cards.iter()).collect();
    let mut dedup = all.clone();
    dedup.sort();
    dedup.dedup();
    assert_eq!(all.len(), dedup.len());
}

#[test]
fn context_menu_actions_change_settings() {
    let _guard = layout_lock();
    let mut h = harness("menu-actions", (2048.0, 1150.0), demo_snapshot(), None, false, false);
    // Right-click the first reading (Tctl, temperatures lead every card) and choose Watch.
    let cpu = app(&mut h).card_rects.iter().find(|(id, _)| id == "cpu").unwrap().1;
    click_secondary(&mut h, first_row_pos(cpu));
    let node = h.get_by_label("Watch");
    node.click();
    h.run_steps(3);
    assert!(app(&mut h).s.sensors.highlighted.contains(&FIRST_CPU_ROW.to_string()));
}

struct LegacyStorage;

impl eframe::Storage for LegacyStorage {
    fn get_string(&self, key: &str) -> Option<String> {
        // Verbatim what the first version stored for the user.
        (key == "hwglance-state-v1").then(|| {
            "(zoom:1.0,split:0.45,sort:Vram,sort_desc:true,section_open:{\"igpu/main\":true,\"cpu/usage\":false,\"cpu/thread\":false,\"cpu/clock\":false,\"cpu/temp\":true},proc_sections:{},procs_visible:true)".to_string()
        })
    }
    fn set_string(&mut self, _key: &str, _value: String) {}
    fn remove_string(&mut self, _key: &str) {}
    fn flush(&mut self) {}
}

#[test]
fn first_run_imports_the_first_versions_eframe_state_into_ui_json() {
    let dir = tmp("legacy");
    let ui_json = dir.join("ui.json");
    let ctx = egui::Context::default();
    let opts = hwglance::app::Opts { settings_path: Some(ui_json.clone()), no_watchers: true, ..Default::default() };
    let app = hwglance::app::App::with_ctx(&ctx, Some(&LegacyStorage), hwglance::app::Mode::Live, opts.clone());
    assert_eq!(app.s.processes.sort.column, "vram");
    assert_eq!(app.s.layout.collapsed_sections.get("cpu/usage"), Some(&true));
    assert_eq!(app.s.layout.collapsed_sections.get("cpu/temp"), Some(&false));
    assert!(!ui_json.exists(), "written only after the debounce");

    // With a ui.json present the old state is ignored.
    let mut s = Settings::default();
    s.processes.sort.column = "memory".into();
    settings::save_atomic(&ui_json, &s, false).unwrap();
    let app = hwglance::app::App::with_ctx(&ctx, Some(&LegacyStorage), hwglance::app::Mode::Live, opts);
    assert_eq!(app.s.processes.sort.column, "memory");
}

/// Frame cost of a full-size window receiving a new snapshot: run with
/// `cargo test --release -p hwglance --test ui perf -- --ignored --nocapture`.
#[test]
#[ignore]
fn perf_frame_cost_at_one_hz() {
    let mut demo = hwglance::demo::Demo::new();
    let mut h = harness("perf", (2048.0, 1150.0), demo.snapshot(1.0), None, false, false);
    h.hover_at(pos2(800.0, 40.0));
    let mut times = Vec::new();
    for i in 0..120 {
        let ctx = h.ctx.clone();
        let snap = demo.snapshot(2.0 + i as f64);
        app(&mut h).link.publish(&ctx, snap);
        let t = std::time::Instant::now();
        h.step();
        let shapes = h.output().shapes.clone();
        let _ = h.ctx.tessellate(shapes, 1.0);
        times.push(t.elapsed().as_secs_f64() * 1000.0);
        // an idle second also costs nothing: no repaint is requested without new data or input
    }
    times.sort_by(|a, b| a.total_cmp(b));
    let avg = times.iter().sum::<f64>() / times.len() as f64;
    println!("frame cost ms: avg {avg:.2}, median {:.2}, p95 {:.2}, max {:.2}", times[60], times[113], times[119]);
}

#[test]
fn focus_expands_scrolls_pins_and_flashes() {
    let mut s = Settings::default();
    s.layout.collapsed_cards.push("cpu".into());
    s.layout.collapsed_sections.insert("cpu/temp".into(), true);
    let mut h = harness("focus", (2048.0, 1150.0), demo_snapshot(), Some(s), false, false);
    let ctx = h.ctx.clone();
    app(&mut h).apply_focus(&ctx, "cpu/temp/tctl", false);
    {
        let a = app(&mut h);
        assert!(!a.s.layout.collapsed_cards.contains(&"cpu".to_string()), "the card is expanded");
        assert_eq!(a.s.layout.collapsed_sections.get("cpu/temp"), Some(&false), "and so is the section");
        assert_eq!(a.scroll_to_reading.as_deref(), Some("cpu/temp/tctl"));
        assert_eq!(a.pinned.as_deref(), Some("cpu/temp/tctl"));
        assert!(a.flash.is_some());
    }
    h.run_steps(6);
    let a = app(&mut h);
    assert!(a.scroll_to_reading.is_none(), "the row was scrolled to");
    assert!(a.pinned_hit.as_ref().is_some_and(|hit| hit.id == "cpu/temp/tctl"), "the row is on screen and its popover is pinned");
    // the flash lasts about 1.2 s
    std::thread::sleep(Duration::from_millis(1300));
    h.run_steps(2);
    assert!(app(&mut h).flash.is_none());
    assert_eq!(app(&mut h).pinned.as_deref(), Some("cpu/temp/tctl"), "the popover stays pinned");
    // Esc unpins
    h.key_press(Key::Escape);
    h.run_steps(2);
    assert!(app(&mut h).pinned.is_none());
}

#[test]
fn a_focus_message_from_the_collector_is_acted_on() {
    let mut h = harness("focus-msg", (2048.0, 1150.0), demo_snapshot(), None, false, false);
    app(&mut h).link.shared.lock().unwrap().focus = Some("gpu0/temp/hotspot".into());
    h.run_steps(5);
    assert_eq!(app(&mut h).pinned.as_deref(), Some("gpu0/temp/hotspot"));
    assert!(app(&mut h).flash.is_some() || app(&mut h).pinned_hit.is_some());
    // an id the collector no longer reports is reported, not crashed on
    app(&mut h).link.shared.lock().unwrap().focus = Some("nope/gone".into());
    h.run_steps(3);
    let toast = app(&mut h).toast.as_ref().map(|t| t.text.clone());
    assert_eq!(toast.as_deref(), Some("That value is not reported right now"), "the toast never prints a reading id");
}

#[test]
fn focus_from_the_command_line_waits_for_the_first_snapshot() {
    let mut h = harness_focus("focus-cli", (2048.0, 1150.0), demo_snapshot(), "gpu0/temp/core");
    assert_eq!(app(&mut h).pinned.as_deref(), Some("gpu0/temp/core"));
    assert!(app(&mut h).pending_focus.is_none());
}

#[test]
fn hovering_a_row_for_350_ms_opens_its_history_popover_and_a_click_pins_it() {
    let _guard = layout_lock();
    let mut h = harness("popover", (2048.0, 1150.0), demo_snapshot(), None, false, false);
    let cpu = app(&mut h).card_rects.iter().find(|(id, _)| id == "cpu").unwrap().1;
    let row = first_row_pos(cpu);
    h.hover_at(row);
    h.run_steps(3);
    assert!(app(&mut h).hover_pop.is_none(), "not yet");
    // A slow machine can see a demo tick move the rows under the pointer, which restarts the
    // 350 ms wait; keep the pointer there for up to 3 s rather than one fixed wait.
    let mut id = None;
    for _ in 0..30 {
        std::thread::sleep(Duration::from_millis(100));
        h.hover_at(row);
        h.run_steps(3);
        id = app(&mut h).hover_pop.as_ref().map(|h| h.id.clone());
        if id.is_some() {
            break;
        }
    }
    assert_eq!(id.as_deref(), Some(FIRST_CPU_ROW));
    // the popover asked for history
    assert!(!app(&mut h).link.take_demo_history().is_empty());
    // moving away closes it
    h.hover_at(pos2(1000.0, 3.0));
    h.run_steps(3);
    assert!(app(&mut h).hover_pop.is_none());
    // a click pins it, a click elsewhere lets go
    click(&mut h, row);
    assert_eq!(app(&mut h).pinned.as_deref(), Some(FIRST_CPU_ROW));
    click(&mut h, pos2(1000.0, 3.0));
    assert!(app(&mut h).pinned.is_none());
}

#[test]
fn legend_chips_jump_to_their_panel() {
    let _guard = layout_lock();
    let mut h = harness("legend", (2048.0, 1150.0), demo_snapshot(), None, false, false);
    // the GPU chip is the second one in the top bar
    h.hover_at(pos2(215.0, 30.0));
    h.run_steps(2);
    assert_eq!(app(&mut h).hover_device, Some(hwglance::ui_kit::Device::Gpu));
    click(&mut h, pos2(215.0, 30.0));
    // the request is consumed by the sensor area in the same or the next pass
    h.run_steps(3);
    assert!(app(&mut h).scroll_to_card.is_none());
}

#[test]
fn switching_to_the_omarchy_font_and_back_keeps_rendering() {
    let _guard = layout_lock();
    let mut h = harness("fonts", (1600.0, 900.0), demo_snapshot(), None, false, false);
    app(&mut h).s.appearance.font = "omarchy".into();
    h.run_steps(6);
    app(&mut h).s.appearance.font = "atkinson".into();
    h.run_steps(6);
    app(&mut h).s.appearance.font_size = 20.0;
    h.run_steps(4);
    assert_eq!(hwglance::theme::base(), 20.0);
    app(&mut h).s.appearance.font_size = 14.5;
    h.run_steps(4);
    assert_eq!(hwglance::theme::base(), 14.5);
}

#[test]
fn the_users_layout_fits_the_window_without_a_horizontal_scrollbar() {
    // Five explicit columns (4 x 461 and 480) made for a wider screen.
    let text = r#"{"layout":{"auto":false,"columns":[{"width":461.0,"cards":["cpu","memory"]},{"width":461.0,"cards":["gpu0"]},{"width":461.0,"cards":["board"]},{"width":461.0,"cards":["igpu","disk/S6XXNJ0R412087B"]},{"width":480.0,"cards":["disk/2514074033BD10771265","disk/S6YJNU0T518330P"]}]}}"#;
    let s: Settings = serde_json::from_str(text).unwrap();
    let mut h = harness("user-layout", (2048.0, 1150.0), repo_fixture(), Some(s), false, false);
    let rects = app(&mut h).card_rects.clone();
    assert!(rects.len() >= 6, "all cards are drawn");
    let right = rects.iter().map(|(_, r)| r.right()).fold(0.0, f32::max);
    assert!(right <= 2048.0 - 8.0, "the rightmost card ends at {right}, inside the window");
    // the saved widths were not touched
    assert_eq!(app(&mut h).s.layout.columns[0].width, 461.0);
}

#[test]
fn a_flagged_legend_chip_jumps_to_the_flagged_row() {
    let _guard = layout_lock();
    // The CPU is throttling at this moment of the demo.
    let (snap, _) = demo_scene(hwglance::demo::T_START + 362.0);
    let mut h = harness("chip-jump", (2048.0, 1150.0), snap.clone(), None, false, false);
    let expected = hwglance::sensors::device_state(&snap, &app(&mut h).s, hwglance::ui_kit::Device::Cpu);
    assert!(expected.mark.is_some() && expected.target.is_some(), "the CPU is flagged: {expected:?}");
    // a quiet device stays a plain panel jump
    let quiet = hwglance::sensors::device_state(&snap, &app(&mut h).s, hwglance::ui_kit::Device::Memory);
    assert_eq!(quiet.mark, None);

    h.hover_at(pos2(165.0, 30.0));
    h.run_steps(2);
    assert_eq!(app(&mut h).hover_device, Some(hwglance::ui_kit::Device::Cpu), "the first chip is the CPU");
    click(&mut h, pos2(165.0, 30.0));
    let a = app(&mut h);
    assert_eq!(a.flash.as_ref().map(|f| f.0.as_str()), expected.target.as_deref(), "the flagged row flashes");
    assert!(a.scroll_to_card.is_none());
    assert!(a.pinned.is_none(), "jumping does not pin the history popover");
}

#[test]
fn card_headers_stay_pinned_while_their_rows_scroll() {
    let mut h = harness("sticky", (1400.0, 560.0), demo_snapshot(), None, false, false);
    assert!(app(&mut h).sticky_heads.is_empty(), "nothing is pinned at the top");
    h.hover_at(pos2(300.0, 200.0));
    h.run_steps(2);
    h.event(egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: egui::vec2(0.0, -260.0), modifiers: egui::Modifiers::NONE, phase: egui::TouchPhase::Move });
    h.run_steps(4);
    let a = app(&mut h);
    assert!(!a.sticky_heads.is_empty(), "scrolled rows have their card header above them");
    let top = a.card_rects.iter().filter(|(id, _)| a.sticky_heads.contains(id)).map(|(_, r)| r.top()).fold(f32::MAX, f32::min);
    assert!(top < 0.0, "the pinned cards' real headers are scrolled away ({top})");
    // scrolling back brings the real headers back
    h.event(egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: egui::vec2(0.0, 600.0), modifiers: egui::Modifiers::NONE, phase: egui::TouchPhase::Move });
    h.run_steps(4);
    assert!(app(&mut h).sticky_heads.is_empty());
}

#[test]
fn before_the_first_snapshot_reset_is_disabled_and_a_skeleton_waits() {
    let dir = tmp("no-snapshot");
    let opts = hwglance::app::Opts { settings_path: Some(dir.join("ui.json")), no_watchers: true, ..Default::default() };
    let mock = dir.join("missing.json");
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1400.0, 600.0)).build_ui_state(
        move |ui, app: &mut Option<hwglance::app::App>| {
            if app.is_none() {
                *app = Some(hwglance::app::App::with_ctx(ui.ctx(), None, hwglance::app::Mode::Mock(mock.clone()), opts.clone()));
                return;
            }
            app.as_mut().unwrap().show(ui);
        },
        None,
    );
    h.run_steps(6);
    assert!(h.query_by_label("Connecting to the collector...").is_some());
    assert!(h.query_by_label("Start collector").is_none(), "a source that cannot fail has nothing to start");
    assert!(h.get_by_label("Reset").accesskit_node().is_disabled(), "nothing to reset yet");
}

fn header_pos(h: &mut H, x_in_list: f32) -> egui::Pos2 {
    let list = app(h).procs_list_rect;
    pos2(list.left() + x_in_list, list.top() - 14.0)
}

#[test]
fn the_column_checklist_opens_on_the_header_and_changes_the_table() {
    let _guard = layout_lock();
    let mut h = harness("columns-menu", (2400.0, 1350.0), demo_snapshot(), None, false, false);
    assert_eq!(app(&mut h).s.current_view().columns, ["pid", "cpu", "gpu", "vram", "memory", "disk", "location"]);
    let at = header_pos(&mut h, 120.0);
    click_secondary(&mut h, at);
    // the checklist lists every optional column, Name is shown but fixed
    for title in ["Threads", "User", "Started", "Encode", "Decode", "Disk", "Location"] {
        assert!(h.query_by_role_and_label(egui::accesskit::Role::CheckBox, title).is_some(), "{title} is in the checklist");
    }
    h.get_by_role_and_label(egui::accesskit::Role::CheckBox, "Threads").click();
    h.run_steps(3);
    assert_eq!(app(&mut h).s.current_view().columns, ["pid", "cpu", "gpu", "vram", "memory", "disk", "threads", "location"]);
    // and a column can be switched off again
    // (the header cell called Disk is a node now too, so the checkbox is asked for by its role)
    h.get_by_role_and_label(egui::accesskit::Role::CheckBox, "Disk").click();
    h.run_steps(3);
    assert!(!app(&mut h).s.current_view().columns.contains(&"disk".to_string()));
}

#[test]
fn the_right_dock_gives_the_panel_the_full_height_and_the_sensors_the_rest() {
    let _guard = layout_lock();
    let mut s = Settings::default();
    s.layout.set_dock(hwglance::settings::Dock::Right);
    let mut h = harness("dock-right", (2400.0, 1350.0), demo_snapshot(), Some(s), false, false);
    let list = app(&mut h).procs_list_rect;
    // the table is on the right and tall: it reaches down toward the bottom of the window
    assert!(list.left() > 1200.0, "the panel is at the right: {list:?}");
    assert!(list.height() > 900.0, "and spans the content height: {}", list.height());
    assert!((list.width() + 24.0 - 500.0).abs() < 8.0, "the compact view is about 500 px wide: {}", list.width() + 24.0);
    // the sensor cards flow into the width that is left, none under the panel
    let rects = app(&mut h).card_rects.clone();
    assert!(rects.len() >= 6);
    let right = rects.iter().map(|(_, r)| r.right()).fold(0.0, f32::max);
    assert!(right <= list.left(), "cards end at {right}, the panel starts at {}", list.left());
    assert!(app(&mut h).s.layout.auto, "no arrangement was frozen");
}

#[test]
fn a_saved_compact_width_below_the_minimum_renders_at_the_minimum() {
    let _guard = layout_lock();
    let mut s = Settings::default();
    s.layout.set_dock(hwglance::settings::Dock::Right);
    s.processes.views.right_compact.width_px = Some(340.0);
    let mut h = harness("compact-min", (2400.0, 1350.0), demo_snapshot(), Some(s), false, false);
    let w = app(&mut h).procs_list_rect.width() + 24.0;
    assert!(w > 340.0 && w < 500.0, "the panel is wider than the saved 340 px: {w}");
    // the saved value is only rewritten when the divider is dragged
    assert_eq!(app(&mut h).s.processes.views.right_compact.width_px, Some(340.0));
}

#[test]
fn the_dock_buttons_switch_the_panel_and_share_one_arrangement() {
    let _guard = layout_lock();
    let mut h = harness("dock-switch", (2400.0, 1350.0), demo_snapshot(), None, false, false);
    assert_eq!(app(&mut h).s.layout.dock(), hwglance::settings::Dock::Bottom);
    h.get_by_label("Dock the process panel at the right").click();
    h.run_steps(4);
    assert_eq!(app(&mut h).s.layout.dock(), hwglance::settings::Dock::Right);
    assert_eq!(app(&mut h).s.layout.process_dock, "right");

    // An edit made while docked right is the one arrangement: it freezes the shared columns
    app(&mut h).edit_layout = true;
    h.run_steps(3);
    let first = app(&mut h).card_rects.iter().find(|(id, _)| id == "cpu").unwrap().1;
    let last = app(&mut h).card_rects.iter().map(|(_, r)| r.left()).fold(0.0, f32::max);
    drag(&mut h, pos2(first.left() + 12.0, first.top() + 18.0), pos2(last + 100.0, first.top() + 24.0));
    let edited = app(&mut h).s.layout.columns.clone();
    assert!(!app(&mut h).s.layout.auto && !edited.is_empty(), "the edit froze the arrangement");

    // back to the bottom: the same columns, in the same order
    app(&mut h).edit_layout = false;
    h.get_by_label("Dock the process panel below the sensors").click();
    h.run_steps(4);
    assert_eq!(app(&mut h).s.layout.dock(), hwglance::settings::Dock::Bottom);
    assert!(!app(&mut h).s.layout.auto);
    assert_eq!(app(&mut h).s.layout.columns, edited);

    // and right again, nothing changed on the way
    h.get_by_label("Dock the process panel at the right").click();
    h.run_steps(4);
    assert_eq!(app(&mut h).s.layout.dock(), hwglance::settings::Dock::Right);
    assert_eq!(app(&mut h).s.layout.columns, edited);
}

#[test]
fn the_table_shows_location_and_scrolls_sideways_only_when_it_must() {
    let _guard = layout_lock();
    // wide window: no horizontal scrolling, Location takes the width that is left
    let mut h = harness("loc-wide", (2400.0, 1350.0), demo_snapshot(), None, false, false);
    assert!(app(&mut h).s.current_view().columns.contains(&"location".to_string()), "the default columns include Location");
    // a narrow right dock with every column: the table scrolls and still renders
    let mut s = Settings::default();
    s.layout.set_dock(hwglance::settings::Dock::Right);
    s.layout.process_view.right = "expanded".into();
    s.processes.views.right_expanded.size = Some(0.3);
    s.processes.views.right_expanded.columns = hwglance::model::SortKey::OPTIONAL.iter().map(|k| k.as_str().to_string()).collect();
    let mut h2 = harness("loc-narrow", (1300.0, 800.0), demo_snapshot(), Some(s), false, false);
    h2.run_steps(3);
    assert_eq!(app(&mut h2).s.current_view().columns.len(), 12);
    assert!(app(&mut h2).procs_list_rect.width() > 100.0);
}

fn right_docked(view: &str) -> Settings {
    let mut s = Settings::default();
    s.layout.set_dock(hwglance::settings::Dock::Right);
    s.layout.process_view.right = view.into();
    s
}

fn panel_width(h: &mut H) -> f32 {
    let list = app(h).procs_list_rect;
    // the table spans the panel's inner width (12 px margins each side)
    list.width() + 24.0
}

#[test]
fn ctrl_e_and_the_header_button_switch_between_the_compact_and_expanded_views() {
    let _guard = layout_lock();
    let mut h = harness("view-switch", (2400.0, 1350.0), demo_snapshot(), Some(right_docked("compact")), false, false);
    let compact_w = panel_width(&mut h);
    assert!((compact_w - 500.0).abs() < 8.0, "{compact_w}");
    // Ctrl+E: expanded, which is wider (a 0.55 share of the content width)
    h.key_press_modifiers(egui::Modifiers::COMMAND, Key::E);
    h.run_steps(3);
    assert_eq!(app(&mut h).s.layout.view(), hwglance::settings::PanelView::Expanded);
    let wide = panel_width(&mut h);
    assert!(wide > compact_w + 400.0, "{wide} vs {compact_w}");
    // and back
    h.key_press_modifiers(egui::Modifiers::COMMAND, Key::E);
    h.run_steps(3);
    assert_eq!(app(&mut h).s.layout.view(), hwglance::settings::PanelView::Compact);
    assert!((panel_width(&mut h) - compact_w).abs() < 1.0);
    // the arrow button in the panel header does the same, with a tooltip saying what it does
    let tip = h.get_by_label("Expand the process panel");
    tip.click();
    h.run_steps(3);
    assert_eq!(app(&mut h).s.layout.view(), hwglance::settings::PanelView::Expanded);
    h.get_by_label("Collapse the process panel").click();
    h.run_steps(3);
    assert_eq!(app(&mut h).s.layout.view(), hwglance::settings::PanelView::Compact);
    // the bottom dock kept its own view throughout
    assert_eq!(app(&mut h).s.layout.process_view.bottom, "compact");
}

#[test]
fn double_clicking_the_divider_toggles_the_view_in_both_docks() {
    let _guard = layout_lock();
    for (name, settings) in [("dbl-right", right_docked("compact")), ("dbl-bottom", Settings::default())] {
        let mut h = harness_with_dt(name, (2400.0, 1350.0), demo_snapshot(), Some(settings), 0.05);
        let right = app(&mut h).s.layout.dock() == hwglance::settings::Dock::Right;
        let list = app(&mut h).procs_list_rect;
        // the divider sits just outside the panel: left of it (right dock) or above it (bottom dock)
        let at = if right { pos2(list.left() - 18.0, 600.0) } else { pos2(1000.0, list.top() - 82.0) };
        let before = app(&mut h).s.layout.view();
        h.event(egui::Event::PointerMoved(at));
        h.run_steps(1);
        // two quick clicks, one frame apart (well inside egui's double-click delay)
        for _ in 0..2 {
            for pressed in [true, false] {
                h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE });
            }
            h.run_steps(1);
        }
        h.run_steps(2);
        assert_eq!(app(&mut h).s.layout.view(), before.toggled(), "{name}");
    }
}

#[test]
fn dragging_the_divider_resizes_only_the_current_view() {
    let _guard = layout_lock();
    let mut h = harness("view-resize", (2400.0, 1350.0), demo_snapshot(), Some(right_docked("compact")), false, false);
    let before = app(&mut h).s.processes.views.clone();
    let list = app(&mut h).procs_list_rect;
    let at = pos2(list.left() - 18.0, 600.0);
    drag(&mut h, at, at - egui::vec2(120.0, 0.0));
    let v = &app(&mut h).s.processes.views;
    let w = v.right_compact.width_px.unwrap();
    assert!(w > 560.0 && w < 640.0, "dragging left widened the compact panel by about 120 px: {w}");
    assert_eq!(v.right_expanded, before.right_expanded, "the other views are untouched");
    assert_eq!(v.bottom_compact, before.bottom_compact);
    assert_eq!(v.bottom_expanded, before.bottom_expanded);
    assert_eq!(v.right_compact.columns, before.right_compact.columns);
}

#[test]
fn the_compact_right_view_shows_few_columns_and_keeps_its_header_usable() {
    let _guard = layout_lock();
    let mut h = harness("compact-header", (2400.0, 1350.0), demo_snapshot(), Some(right_docked("compact")), false, false);
    // the buttons are all there in 500 px: expand, both docks, Hide
    for glyph in ["Expand the process panel", "Dock the process panel at the right", "Dock the process panel below the sensors"] {
        assert!(h.query_by_label(glyph).is_some(), "button {glyph:?}");
    }
    assert!(h.query_by_label_contains("Hide").is_some());
    // the search is an icon, which opens the field on a click and on "/"
    assert!(h.query_by_label("Search processes").is_some(), "the search icon");
    h.get_by_label("Search processes").click();
    h.run_steps(4);
    assert!(app(&mut h).search_open);
    assert!(h.ctx.egui_wants_keyboard_input(), "the field has the focus");
    h.key_press(Key::Escape);
    h.run_steps(3);
    // "/" opens it as well
    h.key_press(Key::Slash);
    h.run_steps(4);
    assert!(h.ctx.egui_wants_keyboard_input());
    // the table shows CPU, GPU and Memory without scrolling sideways
    let list = app(&mut h).procs_list_rect;
    assert!(list.width() > 400.0);
    // while the bottom dock keeps its full toolbar (no icon)
    let b = harness("bottom-header", (2400.0, 1350.0), demo_snapshot(), None, false, false);
    assert!(b.query_by_role_and_label(egui::accesskit::Role::Button, "Search processes").is_none(), "no search icon button");
    assert!(b.query_by_role_and_label(egui::accesskit::Role::SearchInput, "Search processes").is_some(), "the field itself");
}

/// Where the Name divider is: on the header row, at the right edge of the Name column.
fn name_divider(h: &mut H) -> egui::Pos2 {
    let (list, x) = (app(h).procs_list_rect, app(h).name_divider_x);
    pos2(x, list.top() - 14.0)
}

fn double_click(h: &mut H, at: egui::Pos2) {
    h.event(egui::Event::PointerMoved(at));
    h.run_steps(1);
    for _ in 0..2 {
        for pressed in [true, false] {
            h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE });
        }
        h.run_steps(1);
    }
    h.run_steps(2);
}

#[test]
fn dragging_the_name_divider_sets_a_per_view_name_width_without_sorting() {
    let _guard = layout_lock();
    let mut h = harness("name-drag", (2400.0, 1350.0), demo_snapshot(), None, false, false);
    let before = app(&mut h).s.processes.views.clone();
    let sort = app(&mut h).s.processes.sort.clone();
    assert_eq!(before.bottom_compact.name_width, None, "automatic to begin with");
    let at = name_divider(&mut h);
    drag(&mut h, at, at + egui::vec2(-60.0, 0.0));
    let w = app(&mut h).s.processes.views.bottom_compact.name_width.expect("the drag saved a width");
    assert!(w > 400.0 && w < 520.0, "the automatic 560 px shrank by about 60: {w}");
    let moved = name_divider(&mut h);
    assert!((moved.x - (at.x - 60.0)).abs() < 4.0, "the divider followed the pointer: {} vs {}", moved.x, at.x);
    // a drag is not a click on the Name header: the sort is unchanged, and no other view moved
    assert_eq!(app(&mut h).s.processes.sort, sort);
    let v = &app(&mut h).s.processes.views;
    assert_eq!(v.bottom_expanded, before.bottom_expanded);
    assert_eq!(v.right_compact, before.right_compact);
    assert_eq!(v.right_expanded, before.right_expanded);
    // dragging far left stops at the compact minimum; far right stops where the data columns still fit
    let at = name_divider(&mut h);
    drag(&mut h, at, at + egui::vec2(-900.0, 0.0));
    let lo = app(&mut h).s.processes.views.bottom_compact.name_width.unwrap();
    assert!((100.0..=180.0).contains(&lo), "{lo}");
    let at = name_divider(&mut h);
    drag(&mut h, at, at + egui::vec2(1500.0, 0.0));
    let hi = app(&mut h).s.processes.views.bottom_compact.name_width.unwrap();
    let list = app(&mut h).procs_list_rect;
    assert!(hi > 900.0 && hi < list.width() - 300.0, "{hi} in a {} px table keeps the columns visible", list.width());
    // the saved width survives a window shrink without being rewritten
    app(&mut h).s.processes.views.bottom_compact.name_width = Some(600.0);
    h.set_size(egui::vec2(1500.0, 1000.0));
    h.run_steps(4);
    assert_eq!(app(&mut h).s.processes.views.bottom_compact.name_width, Some(600.0));
    assert!(app(&mut h).name_divider_x < app(&mut h).procs_list_rect.right() - 300.0, "Name gave way to the columns");
}

#[test]
fn double_clicking_the_name_divider_fits_the_names() {
    let _guard = layout_lock();
    let mut h = harness_with_dt("name-fit", (2400.0, 1350.0), demo_snapshot(), None, 0.05);
    let at = name_divider(&mut h);
    double_click(&mut h, at);
    let fit = app(&mut h).s.processes.views.bottom_compact.name_width.expect("fit saved a width");
    assert!((120.0..=560.0).contains(&fit), "{fit}");
    assert_eq!(app(&mut h).s.layout.view(), hwglance::settings::PanelView::Compact, "not the view toggle");
    // the divider moved to the fitted width, and fitting again changes nothing
    let at = name_divider(&mut h);
    double_click(&mut h, at);
    let again = app(&mut h).s.processes.views.bottom_compact.name_width.unwrap();
    assert!((again - fit).abs() < 1.0, "{again} vs {fit}");
}

/// The rect of a column header in the process table.
fn header_rect(h: &mut H, title: &str) -> egui::Rect {
    let node = h.query_all_by_role(egui::accesskit::Role::ColumnHeader).find(|n| n.accesskit_node().label().as_deref() == Some(title)).expect("the column header");
    node.rect()
}

#[test]
fn the_right_compact_name_divider_moves_the_boundary_without_resizing_the_panel() {
    let _guard = layout_lock();
    let mut h = harness("name-panel", (2400.0, 1350.0), demo_snapshot(), Some(right_docked("compact")), false, false);
    let before = app(&mut h).s.processes.views.clone();
    let w0 = panel_width(&mut h);
    let (x0, cpu0, mem0) = (app(&mut h).name_divider_x, header_rect(&mut h, "CPU"), header_rect(&mut h, "Memory"));
    // Name starts as wide as the data columns allow, so only narrowing it moves the boundary
    let at = name_divider(&mut h);
    drag(&mut h, at, at + egui::vec2(-90.0, 0.0));
    let w = app(&mut h).s.processes.views.right_compact.data_extra.expect("the drag saved the data columns' extra width");
    assert!((w - 90.0).abs() < 4.0, "{w}");
    assert_eq!(app(&mut h).s.processes.views.right_compact.name_width, None, "the right dock does not use a Name width");
    assert_eq!(app(&mut h).s.processes.views.right_compact.width_px, before.right_compact.width_px, "the panel width is not touched");
    assert!((panel_width(&mut h) - w0).abs() < 1.0, "the panel kept its size");
    assert!((app(&mut h).name_divider_x - (x0 - 90.0)).abs() < 4.0, "the boundary followed the pointer");
    let (cpu1, mem1) = (header_rect(&mut h, "CPU"), header_rect(&mut h, "Memory"));
    assert!((cpu0.left() - cpu1.left() - 90.0).abs() < 4.0, "the data columns start where Name now ends");
    assert!(cpu1.width() > cpu0.width() + 10.0 && mem1.width() > mem0.width() + 10.0, "and share the room Name gave up");
    assert!((mem1.right() - mem0.right()).abs() < 1.0, "the row still ends where it did");
    let v = &app(&mut h).s.processes.views;
    assert_eq!(v.right_expanded, before.right_expanded);
    assert_eq!(v.bottom_compact, before.bottom_compact);
    // dragging back out stops where the data columns are at their fitted widths
    let at = name_divider(&mut h);
    drag(&mut h, at, at + egui::vec2(900.0, 0.0));
    assert!((app(&mut h).name_divider_x - x0).abs() < 1.0, "{} vs {x0}", app(&mut h).name_divider_x);
    assert!((header_rect(&mut h, "CPU").width() - cpu0.width()).abs() < 1.0);
    // and far left stops at the compact minimum, with the panel still the same size
    let at = name_divider(&mut h);
    drag(&mut h, at, at + egui::vec2(-900.0, 0.0));
    let extra = app(&mut h).s.processes.views.right_compact.data_extra.unwrap();
    let list = app(&mut h).procs_list_rect;
    let name_w = app(&mut h).name_divider_x - list.left();
    assert!((100.0..=180.0).contains(&name_w), "Name is at the compact minimum: {name_w}");
    assert!(extra > 90.0, "{extra}");
    assert_eq!(app(&mut h).s.processes.views.right_compact.width_px, before.right_compact.width_px);
    assert!((panel_width(&mut h) - w0).abs() < 1.0);
}

#[test]
fn widening_the_right_panel_after_a_drag_widens_name_and_never_the_data_columns() {
    let _guard = layout_lock();
    let mut s = right_docked("compact");
    s.processes.views.right_compact.width_px = Some(560.0);
    let mut h = harness("name-panel-widen", (2400.0, 1350.0), demo_snapshot(), Some(s), false, false);
    let at = name_divider(&mut h);
    drag(&mut h, at, at + egui::vec2(-70.0, 0.0));
    let extra = app(&mut h).s.processes.views.right_compact.data_extra.expect("the drag saved an extra width");
    assert!(extra > 50.0, "{extra}");
    let cols = ["CPU", "GPU", "Memory"];
    let name_w = |h: &mut H| app(h).name_divider_x - app(h).procs_list_rect.left();
    let (n0, w0) = (name_w(&mut h), panel_width(&mut h));
    let widths0: Vec<f32> = cols.iter().map(|c| header_rect(&mut h, c).width()).collect();
    // widen the panel by 120 px: Name takes all of it
    app(&mut h).s.processes.views.right_compact.width_px = Some(680.0);
    h.run_steps(4);
    let dw = panel_width(&mut h) - w0;
    assert!((dw - 120.0).abs() < 4.0, "the panel widened: {dw}");
    assert!((name_w(&mut h) - n0 - dw).abs() < 2.0, "Name grew by the panel's growth");
    for (c, w) in cols.iter().zip(&widths0) {
        assert!((header_rect(&mut h, c).width() - w).abs() < 0.5, "{c} kept its width");
    }
    assert_eq!(app(&mut h).s.processes.views.right_compact.data_extra, Some(extra), "nothing was rewritten");
    // and narrowing it takes the width from Name, down to the minimum, then the extra gives way
    // without being rewritten
    app(&mut h).s.processes.views.right_compact.width_px = Some(440.0);
    h.run_steps(4);
    assert_eq!(app(&mut h).s.processes.views.right_compact.data_extra, Some(extra));
    let list = app(&mut h).procs_list_rect;
    assert!(app(&mut h).name_divider_x - list.left() >= 100.0, "Name keeps its minimum");
    // the divider's way back: Home gives the data columns their fitted widths
    app(&mut h).s.processes.views.right_compact.data_extra = None;
    app(&mut h).s.processes.views.right_compact.width_px = Some(680.0);
    h.run_steps(4);
    assert!(header_rect(&mut h, "CPU").width() < widths0[0] - 20.0, "no extra, no wider columns");
}

#[test]
fn double_clicking_the_right_compact_name_divider_fits_the_names_and_keeps_the_panel_size() {
    let _guard = layout_lock();
    let mut s = right_docked("compact");
    s.processes.views.right_compact.width_px = Some(900.0);
    let mut h = harness_with_dt("name-panel-fit", (2400.0, 1350.0), demo_snapshot(), Some(s), 0.05);
    let x0 = app(&mut h).name_divider_x;
    let at = name_divider(&mut h);
    double_click(&mut h, at);
    let extra = app(&mut h).s.processes.views.right_compact.data_extra.expect("fit saved the extra width");
    assert!(extra > 100.0, "the over-wide Name shrank to its names and the data columns took the rest: {extra}");
    assert!(app(&mut h).name_divider_x < x0 - 100.0);
    assert_eq!(app(&mut h).s.processes.views.right_compact.width_px, Some(900.0), "the panel did not");
    assert_eq!(app(&mut h).s.layout.view(), hwglance::settings::PanelView::Compact);
}

#[test]
fn the_bar_widget_tab_chooses_what_the_bar_button_shows() {
    let _guard = layout_lock();
    let mut h = harness("bar-display", (1700.0, 1000.0), demo_snapshot(), None, false, true);
    app(&mut h).settings_tab = hwglance::app::SettingsTab::BarWidget;
    h.run_steps(4);
    assert_eq!(app(&mut h).s.tray.bar_display, "icon");
    assert!(h.query_by_label_contains("update about every 2 seconds").is_some(), "the hint");
    for (label, value) in [("CPU usage", "cpu"), ("GPU usage", "gpu"), ("CPU and GPU usage", "cpu_gpu"), ("Icon", "icon")] {
        h.get_by_label(label).click();
        h.run_steps(3);
        assert_eq!(app(&mut h).s.tray.bar_display, value, "{label}");
    }
}

#[test]
fn the_system_theme_counts_only_while_the_setting_says_system() {
    use hwglance::theme::{Palette, Scheme};
    use std::sync::atomic::Ordering;
    let mut h = harness("theme", (1600.0, 900.0), demo_snapshot(), None, false, false);
    let mut step = |h: &mut H, theme: &str, system_light: bool| {
        app(h).s.appearance.theme = theme.into();
        app(h).system_light.store(system_light, Ordering::Relaxed);
        h.run_steps(3);
        app(h).pal.scheme
    };
    assert_eq!(step(&mut h, "system", false), Scheme::Dark);
    assert_eq!(step(&mut h, "system", true), Scheme::Light, "follows the desktop live");
    assert_eq!(h.ctx.theme(), egui::Theme::Light);
    assert_eq!(app(&mut h).pal, Palette::light());
    assert_eq!(step(&mut h, "dark", true), Scheme::Dark, "a forced theme ignores the desktop");
    assert_eq!(h.ctx.theme(), egui::Theme::Dark);
    assert_eq!(step(&mut h, "light", false), Scheme::Light);
    assert_eq!(step(&mut h, "system", false), Scheme::Dark);
}
