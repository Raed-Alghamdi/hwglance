//! The window's destructive actions and core flows: the two-step resets, the critical End task
//! dialog, the reading menu and its threshold fields, adding a threshold in Settings, what
//! follows End task, the popover without a stacked tooltip, and the Settings window. Process
//! kills only ever reach a dummy `sleep` child of the test (or nothing at all).

mod common;

use common::*;
use egui::{pos2, Key, Modifiers};
use egui_kittest::kittest::Queryable;
use hwglance::actions::parse_start_ticks;
use hwglance::app::{KillState, ResetKind, SettingsTab};
use hwglance::model::RowId;
use hwglance::settings::Settings;
use hwglance_proto::{ProcClass, ProcGroup, ProcInfo, Snapshot};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// First reading of the CPU card (temperatures lead every card, and Tctl is their first row).
const TCTL: &str = "cpu/temp/tctl";

fn first_row_pos(card: egui::Rect) -> egui::Pos2 {
    pos2(card.left() + 100.0, card.top() + 36.0 + 20.0 + 28.0 + 14.0)
}

fn cpu_card(h: &mut H) -> egui::Rect {
    app(h).card_rects.iter().find(|(id, _)| id == "cpu").unwrap().1
}

/// The demo snapshot plus one group of one process: the pid of a dummy `sleep` child, shown
/// under `proc_name`. Nothing else is ever signalled.
fn with_dummy(group_id: &str, group_name: &str, proc_name: &str) -> (Snapshot, std::process::Child) {
    let child = std::process::Command::new("sleep").arg("600").spawn().unwrap();
    let pid = child.id() as i32;
    assert!(pid > 1);
    let ticks = parse_start_ticks(&std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap()).unwrap();
    let mut snap = demo_snapshot();
    snap.procs.as_mut().unwrap().push(ProcGroup {
        id: group_id.into(),
        class: ProcClass::App,
        disk_read_bps: None,
        disk_write_bps: None,
        gpu_enc_pct: None,
        gpu_dec_pct: None,
        name: group_name.into(),
        icon: None,
        title: None,
        procs: vec![ProcInfo {
            pid,
            ppid: 1,
            uid: unsafe { libc::getuid() },
            name: proc_name.into(),
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
            // the collector marks critical processes; the dummy plays one when named like it
            critical: (proc_name == "systemd").then(|| "system".to_string()),
        }],
        cpu_pct: 0.0,
        mem_bytes: 1 << 20,
        gpu_pct: None,
        vram_bytes: None,
    });
    (snap, child)
}

fn reap(mut child: std::process::Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn ctrl_r(h: &mut H) {
    h.key_press_modifiers(Modifiers::COMMAND, Key::R);
    h.run_steps(3);
}

// ---------------------------------------------------------------------------
// Item 1: the resets ask twice
// ---------------------------------------------------------------------------

#[test]
fn the_top_bar_reset_asks_twice_and_names_what_it_clears() {
    let mut h = harness("reset-stats", (2048.0, 1150.0), demo_snapshot(), None, false, false);
    assert!(!app(&mut h).link.take_demo_reset());
    h.get_by_label("Reset").click();
    h.run_steps(3);
    assert!(app(&mut h).is_armed(ResetKind::Stats), "the first click only arms it");
    assert!(!app(&mut h).link.take_demo_reset(), "nothing is cleared yet");
    let q = h.query_by_label_contains("Click again to clear low, high, average, throttle history and crossings since ");
    assert!(q.is_some(), "the question names what goes");

    // Cancel drops it
    h.get_by_label("Cancel").click();
    h.run_steps(3);
    assert!(app(&mut h).armed.is_none());
    assert!(!app(&mut h).link.take_demo_reset());

    // the second click does it
    h.get_by_label("Reset").click();
    h.run_steps(3);
    h.get_by_label_contains("Click again to clear low, high, average").click();
    h.run_steps(3);
    assert!(app(&mut h).armed.is_none());
    assert!(app(&mut h).link.take_demo_reset(), "the stats were reset");
}

#[test]
fn ctrl_r_arms_the_same_confirm_and_never_while_typing() {
    let mut h = harness("reset-ctrl-r", (2048.0, 1150.0), demo_snapshot(), None, false, false);
    ctrl_r(&mut h);
    assert!(app(&mut h).is_armed(ResetKind::Stats));
    assert!(!app(&mut h).link.take_demo_reset(), "Ctrl+R does not reset by itself");
    ctrl_r(&mut h);
    assert!(app(&mut h).is_armed(ResetKind::Stats));
    assert!(!app(&mut h).link.take_demo_reset(), "nor does a second Ctrl+R");
    // Esc drops it
    h.key_press(Key::Escape);
    h.run_steps(3);
    assert!(app(&mut h).armed.is_none());

    // in the search field Ctrl+R does nothing at all
    h.key_press(Key::Slash);
    h.run_steps(3);
    assert!(h.ctx.egui_wants_keyboard_input(), "the search field has focus");
    ctrl_r(&mut h);
    assert!(app(&mut h).armed.is_none(), "not armed while typing");
    assert!(!app(&mut h).link.take_demo_reset());
}

#[test]
fn reset_layout_in_the_edit_bar_asks_twice() {
    let mut s = Settings::default();
    s.layout.auto = false;
    s.layout.section_order.insert("cpu".into(), vec!["cpu/temp".into()]);
    let mut h = harness("reset-layout", (2048.0, 1150.0), demo_snapshot(), Some(s), false, false);
    app(&mut h).edit_layout = true;
    h.run_steps(3);
    h.get_by_label("Reset layout").click();
    h.run_steps(3);
    assert!(app(&mut h).is_armed(ResetKind::EditLayout));
    assert!(!app(&mut h).s.layout.auto, "the layout is still the custom one");
    assert!(!app(&mut h).s.layout.section_order.is_empty());
    h.get_by_label_contains("Click again to clear your columns").click();
    h.run_steps(3);
    assert!(app(&mut h).s.layout.auto);
    assert!(app(&mut h).s.layout.section_order.is_empty());
    assert!(app(&mut h).armed.is_none());

    // leaving edit mode drops an armed question
    h.get_by_label("Reset layout").click();
    h.run_steps(3);
    assert!(app(&mut h).armed.is_none() || app(&mut h).is_armed(ResetKind::EditLayout));
    app(&mut h).edit_layout = false;
    h.run_steps(3);
    assert!(app(&mut h).armed.is_none());
}

#[test]
fn settings_resets_for_layout_and_views_ask_twice() {
    let mut s = Settings::default();
    s.layout.auto = false;
    s.layout.reading_order.insert("cpu/temp".into(), vec!["cpu/temp/tctl".into()]);
    s.processes.views.right_expanded.size = Some(0.3);
    let mut h = harness("reset-settings", (2048.0, 1150.0), demo_snapshot(), Some(s), false, true);
    app(&mut h).settings_tab = SettingsTab::Layout;
    h.run_steps(4);
    h.get_by_label("Reset to automatic").click();
    h.run_steps(3);
    assert!(app(&mut h).is_armed(ResetKind::LayoutAuto));
    assert!(!app(&mut h).s.layout.auto, "not yet");
    h.get_by_label_contains("Click again to clear your 0 columns").click();
    h.run_steps(3);
    assert!(app(&mut h).s.layout.auto);
    assert!(app(&mut h).s.layout.reading_order.is_empty());

    app(&mut h).settings_tab = SettingsTab::Processes;
    h.run_steps(4);
    h.get_by_label("Reset views").click();
    h.run_steps(3);
    assert!(app(&mut h).is_armed(ResetKind::ProcViews));
    assert_eq!(app(&mut h).s.processes.views.right_expanded.size, Some(0.3), "not yet");
    h.get_by_label_contains("Click again to reset the sizes").click();
    h.run_steps(3);
    assert_ne!(app(&mut h).s.processes.views.right_expanded.size, Some(0.3));
    assert!(app(&mut h).armed.is_none());

    // a question on another tab is gone when the tab changes
    h.get_by_label("Reset views").click();
    h.run_steps(3);
    assert!(app(&mut h).is_armed(ResetKind::ProcViews));
    app(&mut h).settings_tab = SettingsTab::History;
    h.run_steps(3);
    assert!(app(&mut h).armed.is_none());
}

// ---------------------------------------------------------------------------
// Item 2: ending a critical task
// ---------------------------------------------------------------------------

#[test]
fn ending_a_critical_task_cancels_on_enter_like_force_kill() {
    // A dummy child that the window believes is "systemd" (never pid 1).
    let (snap, mut child) = with_dummy("app:fake-systemd", "Fake systemd", "systemd");
    let mut h = harness("kill-critical", (1600.0, 900.0), snap, None, false, false);
    app(&mut h).selection = Some(RowId::Group("app:fake-systemd".into()));
    h.key_press(Key::Delete);
    h.run_steps(3);
    let pk = app(&mut h).pending_kill.as_ref().expect("a critical task confirms first");
    assert!(pk.warning.is_some());

    // Enter presses Cancel, which has the focus: nothing is sent
    h.key_press(Key::Enter);
    h.run_steps(3);
    assert!(app(&mut h).pending_kill.is_none(), "Enter cancelled the dialog");
    assert!(child.try_wait().unwrap().is_none(), "no signal was sent");
    assert!(h.query_by_label_contains("End task  (Enter)").is_none());

    // Tab then Enter on End task confirms
    h.key_press(Key::Delete);
    h.run_steps(3);
    h.key_press(Key::Tab);
    h.run_steps(3);
    assert!(app(&mut h).pending_kill.is_some(), "Tab alone changes nothing");
    h.key_press(Key::Enter);
    h.run_steps(3);
    assert!(app(&mut h).pending_kill.is_none());
    let status = child.wait().unwrap();
    use std::os::unix::process::ExitStatusExt;
    assert_eq!(status.signal(), Some(libc::SIGTERM));
}

// ---------------------------------------------------------------------------
// Item 3: the reading menu
// ---------------------------------------------------------------------------

#[test]
fn the_reading_menu_stays_open_for_a_typed_threshold_and_closes_on_one_shot_actions() {
    let _guard = layout_lock();
    let mut h = harness("value-menu", (2048.0, 1150.0), demo_snapshot(), None, false, false);
    let row = first_row_pos(cpu_card(&mut h));
    click_secondary(&mut h, row);
    assert!(egui::Popup::is_any_open(&h.ctx), "the menu opened");
    // it opens under the row, not over the row's own value
    let watch = h.get_by_label("Watch").rect();
    assert!(watch.top() >= row.y + 10.0, "the menu starts under the row: {watch:?} vs {row:?}");

    // click the Warning field: the menu must stay open, and the field takes typed input
    let fields: Vec<_> = h.query_all_by_role(egui::accesskit::Role::SpinButton).collect();
    assert!(fields.len() >= 2, "warning and critical fields: {}", fields.len());
    fields[0].click();
    h.run_steps(3);
    assert!(egui::Popup::is_any_open(&h.ctx), "a click inside the menu keeps it open");
    h.event(egui::Event::Text("85".into()));
    h.run_steps(2);
    h.key_press(Key::Enter);
    h.run_steps(3);
    // the window asks the collector, which keeps the limits in thresholds.json; it stores nothing itself
    let sent = app(&mut h).link.take_demo_limits();
    assert_eq!(
        sent.last(),
        Some(&hwglance_proto::ClientMsg::SetThreshold { id: TCTL.into(), warn: Some(85.0), crit: Some(89.0), below: false }),
        "the typed warning limit with the critical one untouched: {sent:?}"
    );
    assert!(!app(&mut h).s.sensors.extra.contains_key("thresholds"), "ui.json gets no limits");
    assert!(egui::Popup::is_any_open(&h.ctx), "still open after typing");
    // until a snapshot shows it, the field keeps what was typed
    let again: Vec<_> = h.query_all_by_role(egui::accesskit::Role::SpinButton).collect();
    assert!(again.len() >= 2, "the fields are still there");

    // the field is drawn with a frame, not as plain text
    save(&mut h, &format!("menu-typing-{}.png", tag()));

    // a one-shot action closes the menu
    h.get_by_label("Watch").click();
    h.run_steps(3);
    assert!(!egui::Popup::is_any_open(&h.ctx), "Watch closed the menu");
    assert!(app(&mut h).s.sensors.highlighted.contains(&TCTL.to_string()));

    // and a click outside closes it too
    click_secondary(&mut h, row);
    assert!(egui::Popup::is_any_open(&h.ctx));
    click(&mut h, pos2(1000.0, 3.0));
    assert!(!egui::Popup::is_any_open(&h.ctx), "a click outside closes the menu");
}

#[test]
fn the_limits_file_is_shown_on_the_alerts_page_with_a_way_to_open_it() {
    let mut h = harness("limits-file", (2048.0, 1150.0), demo_snapshot(), None, false, true);
    app(&mut h).settings_tab = SettingsTab::Alerts;
    h.run_steps(4);
    let path = hwglance::settings::thresholds_path().display().to_string();
    assert!(h.query_by_label(&path).is_some(), "the path of the file is shown");
    assert!(h.query_by_label("Open file").is_some(), "with a button that opens it");
    assert!(h.query_by_label_contains("changes apply within a few seconds").is_some(), "and what to expect from editing it");
    assert!(h.query_by_label("The file has an error").is_none(), "no error while the collector reads it fine");
    assert!(h.query_by_label("Limits file has an error").is_none(), "and no banner");
}

#[test]
fn a_file_the_collector_cannot_read_is_reported_in_the_window() {
    let mut snap = demo_snapshot();
    snap.limits_error = Some("line 3, column 14: invalid type: string \"hot\", expected f64".into());
    let mut h = harness("limits-error", (2048.0, 1150.0), snap, None, false, true);
    app(&mut h).settings_tab = SettingsTab::Alerts;
    h.run_steps(4);
    // the banner above the window, the Alerts page, and a toast that says it once
    assert!(h.query_by_label("Limits file has an error").is_some());
    assert!(h.query_all_by_label_contains("line 3, column 14").count() >= 2, "the banner and the Alerts page carry the position");
    assert!(h.query_by_label("The file has an error").is_some());
    let toast = app(&mut h).toast.as_ref().map(|t| (t.text.clone(), t.ok));
    let (text, ok) = toast.expect("a toast");
    assert!(!ok && text.contains("line 3, column 14") && text.contains("last good limits"), "{text}");
    // while it is broken the editor is off: the collector would not keep a change
    app(&mut h).settings_tab = SettingsTab::Sensors;
    app(&mut h).thr_pick = Some("cpu/usage/total".into());
    h.run_steps(4);
    assert!(h.query_by_label_contains("Paused: thresholds.json has an error").is_some());
    assert!(app(&mut h).link.take_demo_limits().is_empty());
}

#[test]
fn reset_to_default_in_the_menu_asks_the_collector_and_names_its_defaults() {
    let _guard = layout_lock();
    let (default_warn, default_crit) = {
        let snap = demo_snapshot();
        let r = snap.sensors.iter().flat_map(|g| &g.sections).flat_map(|s| &s.readings).find(|r| r.id == TCTL).unwrap().clone();
        (r.warn, r.crit)
    };
    let snap = with_limits(demo_snapshot(), &[(TCTL, Some(60.0), Some(70.0), false)]);
    let mut h = harness("reset-default", (2048.0, 1150.0), snap, None, false, false);
    let row = first_row_pos(cpu_card(&mut h));
    click_secondary(&mut h, row);
    assert!(egui::Popup::is_any_open(&h.ctx));
    // the editor shows the limits in force, and says what the defaults are
    assert!(h.query_by_label_contains("Default: Warns at 80.0 °C, critical at 89.0 °C").is_some());
    assert_eq!((default_warn, default_crit), (Some(80.0), Some(89.0)));
    h.get_by_label("Reset to default").click();
    h.run_steps(3);
    let sent = app(&mut h).link.take_demo_limits();
    assert_eq!(sent.last(), Some(&hwglance_proto::ClientMsg::ResetThreshold { id: TCTL.into() }), "{sent:?}");
    assert!(!egui::Popup::is_any_open(&h.ctx), "the reset closes the menu");
    // the next snapshots may still carry the old limits; the window shows the default meanwhile
    let shown = app(&mut h).snap.clone().unwrap();
    assert!(app(&mut h).thr_pending.custom_ids(&shown).is_empty());
}

#[test]
fn settings_adds_a_threshold_to_any_reading() {
    let mut h = harness("add-threshold", (2048.0, 1150.0), demo_snapshot(), None, false, true);
    app(&mut h).settings_tab = SettingsTab::Sensors;
    h.run_steps(4);
    assert!(app(&mut h).thr_pending.is_empty());
    // search, pick, then the warning and critical fields
    app(&mut h).thr_search = "CPU / Total".into();
    h.run_steps(3);
    let matches = h.query_all_by_label_contains("CPU / Total").count();
    assert!(matches >= 1, "the search lists matching readings");
    h.query_all_by_label_contains("CPU / Total").next().unwrap().click();
    h.run_steps(3);
    let id = app(&mut h).thr_pick.clone().expect("a reading was picked");
    assert!(app(&mut h).thr_search.is_empty());
    assert!(app(&mut h).link.take_demo_limits().is_empty(), "picking alone asks for nothing");

    // tick Warning and type a value
    h.get_by_label("Warning").click();
    h.run_steps(3);
    let fields: Vec<_> = h.query_all_by_role(egui::accesskit::Role::SpinButton).collect();
    assert!(!fields.is_empty(), "the warning field appeared");
    fields[0].click();
    h.run_steps(3);
    h.event(egui::Event::Text("77".into()));
    h.run_steps(2);
    h.key_press(Key::Enter);
    h.run_steps(3);
    let sent = app(&mut h).link.take_demo_limits();
    match sent.last() {
        Some(hwglance_proto::ClientMsg::SetThreshold { id: sent_id, warn, .. }) => assert_eq!((sent_id.as_str(), *warn), (id.as_str(), Some(77.0))),
        other => panic!("expected a SetThreshold, got {other:?}"),
    }
    // and it now shows in the list of custom thresholds
    assert!(h.query_by_label_contains("No custom thresholds yet").is_none());

    // flags have no thresholds, so a search never offers them
    app(&mut h).thr_pick = None;
    app(&mut h).thr_search = "Thermal throttling".into();
    h.run_steps(3);
    assert!(h.query_by_label_contains("No value matches").is_some());
}

// ---------------------------------------------------------------------------
// Item 4: what follows End task
// ---------------------------------------------------------------------------

static ALIVE_ENDED: AtomicBool = AtomicBool::new(true);
fn stub_ended(_: &hwglance::model::TargetProc) -> bool {
    ALIVE_ENDED.load(Ordering::SeqCst)
}

#[test]
fn end_task_reports_ended_or_still_running_in_the_action_row() {
    let (snap, child) = with_dummy("app:dummy-outcome", "Dummy outcome", "sleep");
    let mut h = harness("kill-outcome", (1600.0, 900.0), snap, None, false, false);
    {
        let a = app(&mut h);
        a.alive_check = stub_ended;
        a.kill_check_for = Duration::from_millis(300);
        a.kill_outcome_for = Duration::from_millis(700);
        a.selection = Some(RowId::Group("app:dummy-outcome".into()));
    }
    ALIVE_ENDED.store(true, Ordering::SeqCst);
    h.run_steps(2);
    h.get_by_label("End task").click();
    h.run_steps(3);
    assert_eq!(app(&mut h).kill_watch.as_ref().map(|w| w.state), Some(KillState::Checking));
    assert!(h.query_by_label_contains("Ending ").is_some(), "checking is shown while it waits");

    // the check window passes with the process still there: say so, and highlight Force kill
    std::thread::sleep(Duration::from_millis(350));
    h.run_steps(3);
    assert_eq!(app(&mut h).kill_watch.as_ref().map(|w| w.state), Some(KillState::StillRunning));
    assert!(h.query_by_label_contains("Dummy outcome is still running").is_some());

    // the outcome goes after its time, and a new kill that does end says so
    std::thread::sleep(Duration::from_millis(750));
    h.run_steps(3);
    assert!(app(&mut h).kill_watch.is_none(), "the outcome is not kept for ever");

    ALIVE_ENDED.store(false, Ordering::SeqCst);
    app(&mut h).kill_outcome_for = Duration::from_secs(10);
    // the first End task terminated the dummy; a recorded pid that is gone ends at once
    h.get_by_label("End task").click();
    h.run_steps(3);
    assert_eq!(app(&mut h).kill_watch.as_ref().map(|w| w.state), Some(KillState::Ended));
    assert!(h.query_by_label_contains("Dummy outcome ended").is_some());

    // choosing another row forgets the outcome
    app(&mut h).selection = Some(RowId::Section(ProcClass::App));
    h.run_steps(3);
    assert!(app(&mut h).kill_watch.is_none());
    reap(child);
}

#[test]
fn the_action_row_names_the_selected_target() {
    let (snap, child) = with_dummy("app:dummy-name", "Dummy name", "sleep");
    let mut h = harness("kill-name", (1600.0, 900.0), snap, None, false, false);
    app(&mut h).selection = Some(RowId::Group("app:dummy-name".into()));
    h.run_steps(3);
    let pid = child.id();
    assert!(h.query_by_label_contains(&format!("Dummy name  (pid {pid})")).is_some());
    reap(child);
}

// ---------------------------------------------------------------------------
// Item 12: tooltip and popover
// ---------------------------------------------------------------------------

#[test]
fn a_hovered_row_shows_the_popover_and_no_tooltip_over_it() {
    let _guard = layout_lock();
    // 0.5 s per step, so that the 0.9 s tooltip delay passes within a few steps
    let mut h = harness_with_dt("tooltip", (2048.0, 1150.0), demo_snapshot(), None, 0.5);
    let row = first_row_pos(cpu_card(&mut h));
    h.hover_at(row);
    h.run_steps(2);
    std::thread::sleep(Duration::from_millis(400));
    h.run_steps(4);
    assert_eq!(app(&mut h).hover_pop.as_ref().map(|p| p.id.as_str()), Some(TCTL), "the popover is up");
    assert!(h.query_by_label_contains(TCTL).is_none(), "the tooltip (which prints the id) does not stack on it");
    // what the tooltip said is in the popover
    assert!(h.query_by_label_contains(", average ").is_some(), "the range line moved into the popover");
    assert!(h.query_by_label_contains("Source: k10temp").is_some(), "so did the source");
}

#[test]
fn in_edit_mode_the_tooltip_stays_because_the_popover_is_off() {
    let _guard = layout_lock();
    let mut h = harness_with_dt("tooltip-edit", (2048.0, 1150.0), demo_snapshot(), None, 0.5);
    app(&mut h).edit_layout = true;
    h.run_steps(3);
    let row = first_row_pos(cpu_card(&mut h)) + egui::vec2(60.0, 0.0);
    h.hover_at(row);
    h.run_steps(6);
    assert!(h.query_by_label_contains(TCTL).is_some(), "the tooltip serves where there is no popover");
}

// ---------------------------------------------------------------------------
// Item 39: the Settings window
// ---------------------------------------------------------------------------

fn settings_rect(h: &mut H) -> egui::Rect {
    h.ctx.memory(|m| m.area_rect(egui::Id::new("settings-window"))).expect("the settings window is shown")
}

#[test]
fn settings_opens_under_the_gear_closes_on_esc_and_keeps_its_width_across_tabs() {
    let _guard = layout_lock();
    let mut h = harness("settings-window", (2546.0, 1400.0), demo_snapshot(), None, false, true);
    h.run_steps(4);
    let gear = app(&mut h).gear_rect.expect("the gear was drawn");
    let r = settings_rect(&mut h);
    assert!((r.right() - gear.right()).abs() < 1.5, "right edges line up: window {r:?}, gear {gear:?}");
    assert!(r.top() > gear.bottom() && r.top() < gear.bottom() + 12.0, "just under the gear: {r:?} {gear:?}");

    // the close button is at least 24 px each way
    let close = h.get_by_label("Close settings").rect();
    assert!(close.width() >= 24.0 && close.height() >= 24.0, "{close:?}");

    // the width does not move from tab to tab
    let w0 = r.width();
    for tab in [
        SettingsTab::General,
        SettingsTab::Sensors,
        SettingsTab::Layout,
        SettingsTab::Processes,
        SettingsTab::History,
        SettingsTab::Alerts,
        SettingsTab::BarWidget,
        SettingsTab::Collector,
    ] {
        app(&mut h).settings_tab = tab;
        h.run_steps(4);
        let w = settings_rect(&mut h).width();
        assert!((w - w0).abs() < 0.5, "{tab:?}: {w} against {w0}");
    }

    // Esc closes it
    h.key_press(Key::Escape);
    h.run_steps(3);
    assert!(!app(&mut h).settings_open, "Esc closed Settings");

    // the gear opens it again, again under the gear
    let gear = app(&mut h).gear_rect.unwrap();
    click(&mut h, gear.center());
    assert!(app(&mut h).settings_open);
    h.run_steps(3);
    let r = settings_rect(&mut h);
    assert!((r.right() - gear.right()).abs() < 1.5, "{r:?} {gear:?}");

    // the close button closes it
    h.get_by_label("Close settings").click();
    h.run_steps(3);
    assert!(!app(&mut h).settings_open);
}

#[test]
fn esc_does_not_close_settings_while_a_field_has_the_keyboard() {
    let _guard = layout_lock();
    let mut h = harness("settings-esc-typing", (2048.0, 1150.0), demo_snapshot(), None, false, true);
    app(&mut h).settings_tab = SettingsTab::Sensors;
    h.run_steps(4);
    // focus the threshold search (the only text field of the tab besides the process search)
    // (the search fields are search inputs now)
    let fields: Vec<_> = h.query_all_by_role(egui::accesskit::Role::SearchInput).collect();
    assert!(!fields.is_empty());
    fields.last().unwrap().click();
    h.run_steps(3);
    assert!(h.ctx.egui_wants_keyboard_input());
    h.key_press(Key::Escape);
    h.run_steps(3);
    assert!(app(&mut h).settings_open, "the field took the Esc");
    h.key_press(Key::Escape);
    h.run_steps(3);
    assert!(!app(&mut h).settings_open, "and the next one closes the window");
}

// ---------------------------------------------------------------------------
// Renders (HWGLANCE_SHOTS=<dir>; HWGLANCE_THEME=light for the other palette)
// ---------------------------------------------------------------------------

fn tag() -> &'static str {
    match std::env::var("HWGLANCE_THEME").as_deref() {
        Ok("light") => "light",
        _ => "dark",
    }
}

const SHOT: (f32, f32) = (2546.0, 1400.0);

#[test]
fn shot_resets_armed() {
    let (snap, _) = demo_scene(hwglance::demo::T_START + 30.0);
    let mut h = harness("shot-reset", SHOT, snap, None, true, false);
    h.get_by_label("Reset").click();
    h.run_steps(3);
    save(&mut h, &format!("reset-stats-armed-{}.png", tag()));
}

#[test]
fn shot_reset_layout_armed() {
    let (snap, _) = demo_scene(hwglance::demo::T_START + 30.0);
    let mut s = Settings::default();
    s.layout.auto = false;
    s.layout.section_order.insert("cpu".into(), vec!["cpu/temp".into()]);
    let mut h = harness("shot-reset-layout", SHOT, snap, Some(s), true, false);
    app(&mut h).edit_layout = true;
    h.run_steps(3);
    h.get_by_label("Reset layout").click();
    h.run_steps(3);
    save(&mut h, &format!("reset-layout-armed-{}.png", tag()));
}

#[test]
fn shot_settings_under_the_gear() {
    let (snap, _) = demo_scene(hwglance::demo::T_START + 30.0);
    let mut h = harness("shot-settings", SHOT, snap, None, true, true);
    app(&mut h).settings_tab = SettingsTab::Sensors;
    app(&mut h).thr_search = "Tctl".into();
    h.run_steps(4);
    save(&mut h, &format!("settings-sensors-add-{}.png", tag()));
    app(&mut h).settings_tab = SettingsTab::BarWidget;
    h.run_steps(4);
    save(&mut h, &format!("settings-bar-widget-{}.png", tag()));
}

#[test]
fn shot_critical_kill_dialog() {
    let (snap, child) = with_dummy("app:fake-systemd2", "Fake systemd", "systemd");
    let mut h = harness("shot-critical", SHOT, snap, None, true, false);
    app(&mut h).selection = Some(RowId::Group("app:fake-systemd2".into()));
    h.key_press(Key::Delete);
    h.run_steps(4);
    save(&mut h, &format!("critical-end-task-{}.png", tag()));
    reap(child);
}

static ALIVE_SHOT: AtomicBool = AtomicBool::new(true);
fn stub_shot(_: &hwglance::model::TargetProc) -> bool {
    ALIVE_SHOT.load(Ordering::SeqCst)
}

#[test]
fn shot_selected_process_row_and_outcome() {
    let (mut snap, child) = with_dummy("app:dummy-shot", "Dummy shot", "sleep");
    snap.procs.as_mut().unwrap().last_mut().unwrap().cpu_pct = 30.0;
    let mut h = harness("shot-selected", SHOT, snap, None, true, false);
    {
        let a = app(&mut h);
        a.alive_check = stub_shot;
        a.selection = Some(RowId::Group("app:dummy-shot".into()));
        a.scroll_to_selection = true;
    }
    ALIVE_SHOT.store(true, Ordering::SeqCst);
    h.run_steps(4);
    h.hover_at(pos2(600.0, 5.0));
    h.run_steps(2);
    save(&mut h, &format!("selected-row-{}.png", tag()));
    h.get_by_label("End task").click();
    h.run_steps(3);
    app(&mut h).kill_check_for = Duration::from_millis(1);
    std::thread::sleep(Duration::from_millis(20));
    h.run_steps(4);
    save(&mut h, &format!("outcome-still-running-{}.png", tag()));
    reap(child);
}

#[test]
fn shot_menu_and_popover() {
    let _guard = layout_lock();
    let (snap, hist) = demo_scene(hwglance::demo::T_START + 140.0);
    let mut h = harness("shot-menu", SHOT, snap, None, true, false);
    let ctx = h.ctx.clone();
    app(&mut h).link.publish_history(&ctx, hist);
    h.run_steps(3);
    // the pinned popover of an alert on the CPU card's first row
    app(&mut h).pinned = Some(TCTL.into());
    h.hover_at(pos2(5.0, 5.0));
    h.run_steps(5);
    save(&mut h, &format!("popover-pinned-{}.png", tag()));
    app(&mut h).pinned = None;
    h.run_steps(3);
    // the reading menu
    let row = first_row_pos(cpu_card(&mut h));
    click_secondary(&mut h, row);
    save(&mut h, &format!("reading-menu-{}.png", tag()));
}
