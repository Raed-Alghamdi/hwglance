//! CSV logging in the window: the "Log to file" menu item, the Logging tab and the top bar's chip.
//! The collector does the writing (and has its own tests); here the window only chooses, shows
//! the status it is told, and saves the choice.

mod common;

use common::*;
use egui::{pos2, Key};
use egui_kittest::kittest::{NodeT, Queryable};
use hwglance::app::{ResetKind, SettingsTab};
use hwglance::settings::{LogPrefs, Settings};
use hwglance_proto::{LogStatus, Snapshot};

const TCTL: &str = "cpu/temp/tctl";

fn status(file: Option<&str>, columns: u32, error: Option<&str>) -> LogStatus {
    LogStatus { file: file.map(str::to_string), columns, interval_s: 5, error: error.map(str::to_string) }
}

fn logging_snapshot(st: LogStatus) -> Snapshot {
    let mut snap = demo_snapshot();
    snap.logging = Some(st);
    snap
}

fn logging_tab(h: &mut H) {
    app(h).settings_open = true;
    app(h).settings_tab = SettingsTab::Logging;
    h.run_steps(4);
}

#[test]
fn the_top_bar_has_no_logging_chip_with_demo_data_or_while_the_collector_is_not_logging() {
    let h = harness("log-chip-none", (1800.0, 1000.0), demo_snapshot(), None, false, false);
    assert!(h.query_by_label("Logging").is_none());
    assert!(h.query_by_label("Log error").is_none());
}

#[test]
fn the_chip_says_logging_and_opens_the_logging_tab() {
    let st = status(Some("/home/u/logs/hwglance-2026-10-12_210433.csv"), 3, None);
    let mut h = harness("log-chip", (1800.0, 1000.0), logging_snapshot(st), None, false, false);
    let chip = h.get_by_label("Logging");
    chip.click();
    h.run_steps(4);
    assert!(app(&mut h).settings_open && app(&mut h).settings_tab == SettingsTab::Logging);
    // the tab tells the same thing in full, with the path
    assert!(h.query_by_label_contains("Logging 3 values every 5 s to").is_some());
    assert!(h.query_by_label("/home/u/logs/hwglance-2026-10-12_210433.csv").is_some());
}

#[test]
fn an_error_turns_the_chip_into_log_error_and_the_tab_shows_it() {
    let st = status(None, 0, Some("cannot create /root/logs: Permission denied"));
    let mut h = harness("log-chip-error", (1800.0, 1000.0), logging_snapshot(st), None, false, false);
    assert!(h.query_by_label("Logging").is_none());
    h.get_by_label("Log error").click();
    h.run_steps(4);
    assert_eq!(app(&mut h).settings_tab, SettingsTab::Logging);
    assert!(h.query_by_label("cannot create /root/logs: Permission denied").is_some());
}

#[test]
fn the_tab_says_when_nothing_is_logged_and_when_the_collector_has_not_caught_up() {
    let mut h = harness("log-tab-off", (1800.0, 1000.0), demo_snapshot(), None, false, true);
    logging_tab(&mut h);
    assert!(h.query_by_label("Not logging").is_some());
    assert!(h.query_by_label("Stop logging").is_some());
    // chosen here, not reported yet: within a couple of seconds
    app(&mut h).s.logging.values.push(TCTL.into());
    h.run_steps(3);
    assert!(h.query_by_label_contains("Logging starts within a few seconds").is_some());
    // a collector from before logging existed would never start
    let mut old = demo_snapshot();
    old.proto = hwglance_proto::PROTO_VERSION - 1;
    let mut h = harness("log-tab-old", (1800.0, 1000.0), old, None, false, true);
    logging_tab(&mut h);
    app(&mut h).s.logging.values.push(TCTL.into());
    h.run_steps(3);
    assert!(h.query_by_label_contains("older than this window and does not write logs").is_some());
}

#[test]
fn the_right_click_menu_ticks_a_value_for_logging_and_says_where_the_file_goes() {
    let _guard = layout_lock();
    let mut h = harness("log-menu", (2048.0, 1150.0), demo_snapshot(), None, false, false);
    let card = app(&mut h).card_rects.iter().find(|(id, _)| id == "cpu").unwrap().1;
    let row = pos2(card.left() + 100.0, card.top() + 36.0 + 20.0 + 28.0 + 14.0);
    click_secondary(&mut h, row);
    assert!(egui::Popup::is_any_open(&h.ctx));
    h.get_by_label("Log to file").click();
    h.run_steps(3);
    assert_eq!(app(&mut h).s.logging.values, [TCTL], "the reading is ticked");
    assert!(!egui::Popup::is_any_open(&h.ctx), "and the menu closes");
    let toast = app(&mut h).toast.as_ref().map(|t| (t.text.clone(), t.ok)).expect("a toast on the first value");
    assert!(toast.1 && toast.0.starts_with("Logging CPU Tctl to "), "{toast:?}");
    // ticked again: unticked, and no toast this time
    app(&mut h).toast = None;
    click_secondary(&mut h, row);
    h.get_by_label("Log to file").click();
    h.run_steps(3);
    assert!(app(&mut h).s.logging.values.is_empty());
    assert!(app(&mut h).toast.is_none());
    // a second value does not repeat the toast
    app(&mut h).s.logging.values.push("gpu0/load/core".into());
    click_secondary(&mut h, row);
    h.get_by_label("Log to file").click();
    h.run_steps(3);
    assert_eq!(app(&mut h).s.logging.values, ["gpu0/load/core", TCTL], "tick order");
    assert!(app(&mut h).toast.is_none());
}

#[test]
fn while_everything_is_logged_the_menu_item_is_ticked_and_cannot_be_changed() {
    let _guard = layout_lock();
    let s = Settings { logging: LogPrefs { all: true, ..LogPrefs::default() }, ..Settings::default() };
    let mut h = harness("log-menu-all", (2048.0, 1150.0), demo_snapshot(), Some(s), false, false);
    let card = app(&mut h).card_rects.iter().find(|(id, _)| id == "cpu").unwrap().1;
    click_secondary(&mut h, pos2(card.left() + 100.0, card.top() + 36.0 + 20.0 + 28.0 + 14.0));
    let item = h.get_by_label("Log to file");
    assert!(item.accesskit_node().is_disabled(), "disabled");
    assert_eq!(item.accesskit_node().toggled(), Some(egui::accesskit::Toggled::True), "and shown ticked");
    item.click();
    h.run_steps(3);
    assert!(app(&mut h).s.logging.values.is_empty());
}

#[test]
fn stop_logging_clears_the_choice_and_can_be_undone() {
    let s = Settings { logging: LogPrefs { all: true, values: vec![TCTL.into(), "gone/x".into()], folder: "/tmp/x".into(), interval_seconds: 10, ..LogPrefs::default() }, ..Settings::default() };
    let mut h = harness("log-stop", (1800.0, 1000.0), demo_snapshot(), Some(s), false, true);
    logging_tab(&mut h);
    // a value the sensors no longer report is listed by its id
    assert!(h.get_by_label("Stop logging gone/x (not reported now)").accesskit_node().is_disabled(), "the list is greyed while all is on");
    assert!(h.query_by_label("gone/x (not reported now)").is_some(), "but still shown");
    h.get_by_label("Stop logging").scroll_to_me();
    h.run_steps(3);
    h.get_by_label("Stop logging").click();
    h.run_steps(3);
    let l = app(&mut h).s.logging.clone();
    assert!(!l.active() && l.values.is_empty());
    assert_eq!((l.folder.as_str(), l.interval_seconds), ("/tmp/x", 10), "the folder and interval stay");
    assert!(app(&mut h).undo_offer(ResetKind::StopLogging).is_some());
    h.get_by_label("Undo reset").click();
    h.run_steps(3);
    let l = app(&mut h).s.logging.clone();
    assert!(l.all && l.values == [TCTL, "gone/x"]);
}

#[test]
fn values_are_listed_by_name_and_removed_one_at_a_time() {
    let s = Settings { logging: LogPrefs { values: vec![TCTL.into(), "gone/x".into()], ..LogPrefs::default() }, ..Settings::default() };
    let mut h = harness("log-values", (1800.0, 1000.0), demo_snapshot(), Some(s), false, true);
    logging_tab(&mut h);
    assert!(h.query_by_label("CPU / Tctl").is_some());
    assert!(h.query_by_label("gone/x (not reported now)").is_some());
    h.get_by_label("Stop logging CPU / Tctl").click();
    h.run_steps(3);
    assert_eq!(app(&mut h).s.logging.values, ["gone/x"], "an id of a sensor that is not here stays");
    assert!(h.query_by_label_contains("About").is_some(), "the size estimate");
}

#[test]
fn the_folder_text_reaches_the_settings_on_enter_and_not_per_keystroke() {
    let mut h = harness("log-folder", (1800.0, 1000.0), demo_snapshot(), None, false, true);
    logging_tab(&mut h);
    h.get_by_label("Folder for the log files").click();
    h.run_steps(2);
    h.event(egui::Event::Text("/tmp/hw".into()));
    h.run_steps(3);
    assert_eq!(app(&mut h).s.logging.folder, "", "half-typed text is not given to the collector");
    assert_eq!(app(&mut h).log_folder_edit.as_deref(), Some("/tmp/hw"));
    h.key_press(Key::Enter);
    h.run_steps(3);
    assert_eq!(app(&mut h).s.logging.folder, "/tmp/hw");
    assert!(app(&mut h).log_folder_edit.is_none());
    // Default goes back to the default folder
    h.get_by_label("Default").click();
    h.run_steps(3);
    assert_eq!(app(&mut h).s.logging.folder, "");
    // typed and then the tab changed: the text is kept, not lost
    h.get_by_label("Folder for the log files").click();
    h.run_steps(2);
    h.event(egui::Event::Text("/tmp/other".into()));
    h.run_steps(2);
    app(&mut h).settings_tab = SettingsTab::General;
    h.run_steps(3);
    assert_eq!(app(&mut h).s.logging.folder, "/tmp/other");
}

#[test]
fn a_relative_folder_is_called_out_in_the_tab() {
    let s = Settings { logging: LogPrefs { folder: "logs/here".into(), ..LogPrefs::default() }, ..Settings::default() };
    let mut h = harness("log-relative", (1800.0, 1000.0), demo_snapshot(), Some(s), false, true);
    logging_tab(&mut h);
    assert!(h.query_by_label_contains("not an absolute path").is_some());
}

#[test]
fn compressing_finished_files_is_on_by_default_and_can_be_switched_off() {
    let mut h = harness("log-compress", (1800.0, 1000.0), demo_snapshot(), None, false, true);
    logging_tab(&mut h);
    assert!(app(&mut h).s.logging.compress, "new installs compress");
    h.get_by_label("Compress finished files (gzip)").click();
    h.run_steps(3);
    assert!(!app(&mut h).s.logging.compress);
}

#[test]
fn the_age_limit_is_off_by_default_and_a_choice_comes_with_a_warning() {
    let mut h = harness("log-keep", (1800.0, 1000.0), demo_snapshot(), None, false, true);
    logging_tab(&mut h);
    assert_eq!(app(&mut h).s.logging.keep_days, 0);
    assert!(h.query_by_label_contains("deleted for good").is_none(), "no warning while everything is kept");
    assert!(h.query_by_label("Delete logs older than").is_some());
    app(&mut h).s.logging.keep_days = 30;
    h.run_steps(3);
    assert!(h.query_by_label_contains("older than 30 days").is_some());
    assert!(h.query_by_label_contains("deleted for good").is_some());
}

#[test]
fn the_tab_counts_the_log_files_of_the_folder_and_nothing_else() {
    let dir = std::env::temp_dir().join(format!("hwg-log-usage-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(dir.join("hwglance-2026-10-01_080000.csv"), vec![b'x'; 1024 * 1024]).unwrap();
    std::fs::write(dir.join("hwglance-2026-10-02_080000.csv.gz"), vec![b'x'; 1024 * 1024]).unwrap();
    std::fs::write(dir.join("notes.txt"), vec![b'x'; 5_000_000]).unwrap();
    std::fs::write(dir.join("hwglance-backup.csv"), vec![b'x'; 5_000_000]).unwrap();
    std::fs::write(dir.join("sub").join("hwglance-2026-10-03_080000.csv"), vec![b'x'; 5_000_000]).unwrap();
    let s = Settings { logging: LogPrefs { folder: dir.to_string_lossy().into_owned(), ..LogPrefs::default() }, ..Settings::default() };
    let mut h = harness("log-usage", (1800.0, 1000.0), demo_snapshot(), Some(s), false, true);
    logging_tab(&mut h);
    assert!(h.query_by_label("This folder holds 2 log files, 2.0 MB.").is_some());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_size_estimate_adds_the_compressed_figure_only_while_compression_is_on() {
    let s = Settings { logging: LogPrefs { values: vec![TCTL.into()], ..LogPrefs::default() }, ..Settings::default() };
    let mut h = harness("log-estimate", (1800.0, 1000.0), demo_snapshot(), Some(s), false, true);
    logging_tab(&mut h);
    assert!(h.query_by_label_contains("once compressed").is_some());
    app(&mut h).s.logging.compress = false;
    h.run_steps(3);
    assert!(h.query_by_label_contains("About").is_some());
    assert!(h.query_by_label_contains("once compressed").is_none());
}
