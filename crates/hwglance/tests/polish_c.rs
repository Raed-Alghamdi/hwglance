//! Polish of batch C in the window: the notice about an unreadable ui.json, the process panel's
//! five-row floor, the top bar at narrow widths, where the kill dialog opens, the action row's
//! right-to-left names and the Edit layout button. Writes PNGs into `HWGLANCE_SHOTS` when it is
//! set (a `light` variant under `HWGLANCE_THEME=light`).

mod common;

use common::*;
use egui_kittest::kittest::Queryable;
use hwglance::actions::Signal;
use hwglance::model::RowId;
use hwglance::settings::Settings;
use std::time::Duration;

fn theme() -> String {
    std::env::var("HWGLANCE_THEME").unwrap_or_else(|_| "dark".into())
}

#[test]
fn an_unreadable_ui_json_shows_a_dismissible_notice_that_names_the_saved_copy() {
    let _guard = layout_lock();
    let mut h = harness_with_ui_json("corrupt-ui", (1600.0, 900.0), demo_snapshot(), "{ this is not json", false);
    let path = app(&mut h).settings_path.clone();
    let bad = path.with_file_name("ui.json.bad");
    // before anything is saved the notice says where the copy will go
    assert!(h.query_by_label_contains("could not be read").is_some(), "the notice is up at the start");
    assert!(h.query_by_label_contains("ui.json.bad").is_some(), "it names the copy");
    assert!(h.query_by_label_contains("when the first change is saved").is_some());
    assert!(!bad.exists(), "nothing is copied before a save");

    // the first change is saved: the unreadable file is kept and the notice says so
    app(&mut h).s.appearance.font_size = 16.0;
    h.run_steps(2);
    std::thread::sleep(Duration::from_millis(700));
    h.run_steps(3);
    assert!(bad.exists(), "the unreadable file was kept");
    assert_eq!(std::fs::read_to_string(&bad).unwrap(), "{ this is not json");
    assert!(h.query_by_label_contains("is saved as").is_some(), "the notice now names the saved copy");
    assert!(h.query_by_label_contains("when the first change is saved").is_none());
    save(&mut h, &format!("corrupt-notice-{}.png", theme()));

    // dismissed, it stays gone
    h.get_by_label("Dismiss").click();
    h.run_steps(3);
    assert!(h.query_by_label_contains("could not be read").is_none());
    app(&mut h).s.appearance.font_size = 15.0;
    h.run_steps(2);
    std::thread::sleep(Duration::from_millis(700));
    h.run_steps(3);
    assert!(h.query_by_label_contains("could not be read").is_none(), "one time only");
}

#[test]
fn a_readable_ui_json_shows_no_notice() {
    let mut h = harness("readable-ui", (1600.0, 900.0), demo_snapshot(), Some(Settings::default()), false, false);
    assert!(h.query_by_label_contains("could not be read").is_none());
    assert!(app(&mut h).corrupt_notice.is_none());
}

#[test]
fn the_bottom_panel_lists_about_five_rows_at_1200_by_700() {
    let _guard = layout_lock();
    let mut h = harness("narrow-rows", (1200.0, 700.0), demo_snapshot(), Some(Settings::default()), true, false);
    let list = app(&mut h).procs_list_rect;
    assert!(list.height() >= 5.0 * 26.0 - 6.0, "the list has {} px, about {:.1} rows", list.height(), list.height() / 28.0);
    save(&mut h, &format!("narrow-1200x700-{}.png", theme()));
}

#[test]
fn the_top_bar_drops_its_extras_before_anything_overlaps() {
    let _guard = layout_lock();
    let wide = {
        let h = harness("bar-wide", (2200.0, 900.0), demo_snapshot(), Some(Settings::default()), false, false);
        h.query_by_label_contains("Since").is_some()
    };
    assert!(wide, "a wide bar keeps the Since time");
    for width in [900.0, 720.0, 600.0] {
        let mut h = harness("bar-narrow", (width, 800.0), demo_snapshot(), Some(Settings::default()), true, false);
        assert!(h.query_by_label_contains("Since").is_none(), "{width}: Since went first");
        // the buttons stay on screen, in order, clear of one another
        let edit = h.get_by_label("Edit layout").rect();
        let reset = h.get_by_label("Reset").rect();
        assert!(edit.right() <= width + 0.5, "{width}: {edit:?}");
        assert!(reset.right() <= edit.left(), "{width}: {reset:?} {edit:?}");
        save(&mut h, &format!("topbar-{}-{}.png", width as u32, theme()));
    }
}

#[test]
fn edit_layout_is_a_framed_button_like_the_others() {
    let mut h = harness("edit-button", (1600.0, 900.0), demo_snapshot(), Some(Settings::default()), false, false);
    assert!(h.query_by_role_and_label(egui::accesskit::Role::Button, "Edit layout").is_some());
    h.get_by_label("Edit layout").click();
    h.run_steps(3);
    assert!(app(&mut h).edit_layout, "it still toggles");
}

#[test]
fn the_kill_dialog_opens_just_above_the_process_panels_action_row() {
    let _guard = layout_lock();
    let mut h = harness("kill-place", (1600.0, 1000.0), demo_snapshot(), Some(Settings::default()), true, false);
    let first = app(&mut h).groups().first().map(|g| g.id.clone()).unwrap();
    app(&mut h).selection = Some(RowId::Group(first));
    app(&mut h).request_kill(None, Signal::Kill);
    h.run_steps(4);
    let actions = app(&mut h).actions_rect.expect("the panel drew its action row");
    let cancel = h.get_by_label("Cancel  (Esc)").rect();
    assert!(cancel.bottom() <= actions.top(), "above the action row: {cancel:?} vs {actions:?}");
    assert!(actions.top() - cancel.bottom() < 160.0, "close to it: {cancel:?} vs {actions:?}");
    assert!(cancel.left() >= actions.left() - 1.0 && cancel.left() < actions.left() + 400.0, "over its left end: {cancel:?}");
    save(&mut h, &format!("kill-dialog-{}.png", theme()));
}

#[test]
fn the_action_row_prints_a_right_to_left_name_in_visual_order() {
    let _guard = layout_lock();
    let (snap, _) = demo_scene(hwglance::demo::T_START + 30.0);
    let mut snap = with_arabic_groups(snap);
    let name = "محادثة مع أحمد";
    let shaped = hwglance::text::shape(name).into_owned();
    assert_ne!(shaped, name, "the name is several words, so the visual order differs");
    snap.procs.as_mut().unwrap().iter_mut().find(|g| g.id == ARABIC_APP_ID).unwrap().name = name.into();
    let mut h = harness("action-bidi", (2000.0, 1000.0), snap, Some(Settings::default()), false, false);
    app(&mut h).selection = Some(RowId::Group(ARABIC_APP_ID.into()));
    h.run_steps(4);
    assert!(h.query_by_label_contains(&shaped).is_some(), "the action row shows the visual order");
}

#[test]
fn a_click_on_the_sources_chip_opens_settings_on_the_collector_tab() {
    let _guard = layout_lock();
    let mut h = harness("sources-click", (1600.0, 900.0), demo_snapshot(), Some(Settings::default()), false, false);
    assert!(!app(&mut h).settings_open);
    let chip = app(&mut h).sources_rect.expect("the chip is drawn once there is data");
    click(&mut h, chip.center());
    h.run_steps(3);
    assert!(app(&mut h).settings_open, "Settings opened");
    assert_eq!(app(&mut h).settings_tab, hwglance::app::SettingsTab::Collector);
}
