//! Stage 7 hardening: the flows that fail in real use, driven in the window. Each test says what
//! went wrong before and what the window does now.

mod common;

use common::*;
use egui_kittest::kittest::{NodeT, Queryable};
use hwglance::settings::Settings;
use std::time::Duration;

const SIZE: (f32, f32) = (1500.0, 900.0);

fn dark() -> Settings {
    let mut s = Settings::default();
    s.appearance.theme = "dark".into();
    s
}

fn scene(name: &str) -> H {
    harness(name, SIZE, demo_snapshot(), Some(dark()), false, false)
}

/// Let the 0.5 s save debounce pass in real time, then draw.
fn after_debounce(h: &mut H) {
    std::thread::sleep(Duration::from_millis(650));
    h.run_steps(3);
}

#[test]
fn a_settings_folder_that_cannot_be_written_shows_a_banner_and_keeps_trying() {
    use std::os::unix::fs::PermissionsExt;
    let _guard = layout_lock();
    let mut h = scene("h-save-fail");
    let path = app(&mut h).settings_path.clone();
    let dir = path.parent().unwrap().to_path_buf();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o500)).unwrap();
    let may_write = std::fs::File::create(dir.join("probe")).is_ok();

    app(&mut h).s.processes.resort_seconds = 7.0;
    h.run_steps(2);
    after_debounce(&mut h);
    if may_write {
        // root can write anywhere: there is no failure to show
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        return;
    }
    let why = app(&mut h).save_error.clone();
    assert!(why.as_deref().is_some_and(|w| w.contains("not writable")), "{why:?}");
    assert!(h.query_by_label_contains("Settings not saved").is_some(), "the banner names the problem");
    assert!(h.query_by_label("Try again").is_some());
    // the change is still in effect, only unsaved
    assert_eq!(app(&mut h).s.processes.resort_seconds, 7.0);
    assert_ne!(hwglance::settings::load(&path).settings.processes.resort_seconds, 7.0);

    // the folder comes back; "Try again" saves at once and the banner goes
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    h.get_by_label("Try again").click();
    h.run_steps(2);
    after_debounce(&mut h);
    assert!(app(&mut h).save_error.is_none());
    assert!(h.query_by_label_contains("Settings not saved").is_none());
    assert_eq!(hwglance::settings::load(&path).settings.processes.resort_seconds, 7.0);
}

#[test]
fn a_damaged_resort_interval_does_not_panic_the_list() {
    // 1e30 seconds used to reach Duration::from_secs_f32 unclamped, which panics
    let text = r#"{"version":1,"appearance":{"theme":"dark"},"processes":{"resort_seconds":1e30,"smoothing_seconds":-4}}"#;
    let mut h = harness_with_ui_json("h-resort", SIZE, demo_snapshot(), text, false);
    h.run_steps(3);
    assert!(app(&mut h).s.processes.resort_seconds <= 300.0);
    assert!(!app(&mut h).rows.is_empty());
}

#[test]
fn a_dismissed_polkit_prompt_says_so_in_the_action_row_and_keeps_the_row() {
    use hwglance::app::KillState;
    use hwglance::model::RowId;
    let _guard = layout_lock();
    let mut h = scene("h-polkit");
    let row = app(&mut h).rows.iter().find(|r| matches!(r.id, RowId::Group(_))).map(|r| r.id.clone()).expect("a group row");
    {
        let a = app(&mut h);
        a.selection = Some(row.clone());
        // another user's process: the answer from pkexec has not come yet
        a.kill_watch = Some(hwglance::app::KillWatch {
            row: row.clone(),
            name: "Root thing".into(),
            signal: hwglance::actions::Signal::Term,
            procs: vec![],
            state: KillState::Waiting,
            since: std::time::Instant::now(),
            message: None,
        });
    }
    h.run_steps(3);
    assert_eq!(app(&mut h).kill_watch.as_ref().map(|w| w.state), Some(KillState::Waiting));
    // while the prompt is open, End task and Force kill on the same row do not stack another one
    app(&mut h).request_kill(None, hwglance::actions::Signal::Term);
    app(&mut h).request_kill(None, hwglance::actions::Signal::Kill);
    assert!(app(&mut h).pending_kill.is_none());
    assert_eq!(app(&mut h).toast.as_ref().map(|t| t.text.as_str()), Some("Still waiting for the authentication prompt"));
    // polkit's prompt was dismissed: the answer arrives on the channel
    app(&mut h).kill_tx.send(("Authentication dismissed, nothing was ended".to_string(), false)).unwrap();
    h.run_steps(3);
    let w = app(&mut h).kill_watch.as_ref().expect("the outcome stays");
    assert_eq!(w.state, KillState::Failed);
    assert_eq!(w.message.as_deref(), Some("Authentication dismissed, nothing was ended"));
    // shown in the action row as well as toasted
    let row_rect = app(&mut h).actions_rect.expect("the action row");
    let in_row = h.query_all_by_label_contains("Authentication dismissed").filter(|n| row_rect.contains_rect(n.rect().shrink(1.0))).count();
    assert_eq!(in_row, 1, "the action row says it");
}

// ---------------------------------------------------------------------------------------------
// Drag-only actions (WCAG 2.5.7): every drag has a keyboard path
// ---------------------------------------------------------------------------------------------

use egui::accesskit::Role;
use egui::{Key, Modifiers};
use hwglance::settings::Dock;

fn press(h: &mut H, key: Key) {
    h.key_press(key);
    h.run_steps(3);
}

fn shift_press(h: &mut H, key: Key) {
    h.key_press_modifiers(Modifiers::SHIFT, key);
    h.run_steps(3);
}

/// Top (or left) edge of the named splitter.
fn splitter(h: &H, name: &str) -> egui::Rect {
    h.get_by_role_and_label(Role::Splitter, name).rect()
}

fn focus_splitter(h: &mut H, name: &str) {
    h.get_by_role_and_label(Role::Splitter, name).focus();
    h.run_steps(3);
}

#[test]
fn the_bottom_panel_divider_moves_by_arrow_keys_and_home_restores_the_default() {
    let _guard = layout_lock();
    let mut s = dark();
    s.processes.views.bottom_compact.size = Some(0.5);
    let mut h = harness("h-div-bottom", SIZE, demo_snapshot(), Some(s), false, false);
    let name = "Resize the process panel";
    focus_splitter(&mut h, name);
    let y0 = splitter(&h, name).center().y;
    // Up moves the divider up, so the panel grows, by 4 px; Shift makes it 32
    press(&mut h, Key::ArrowUp);
    let y1 = splitter(&h, name).center().y;
    assert!((y0 - y1 - 4.0).abs() < 0.6, "up 4 px: {y0} -> {y1}");
    shift_press(&mut h, Key::ArrowUp);
    let y2 = splitter(&h, name).center().y;
    assert!((y1 - y2 - 32.0).abs() < 0.6, "up 32 px: {y1} -> {y2}");
    press(&mut h, Key::ArrowDown);
    shift_press(&mut h, Key::ArrowDown);
    let y3 = splitter(&h, name).center().y;
    assert!((y3 - (y2 + 36.0)).abs() < 1.0, "down 4 and 32: {y2} -> {y3}");
    assert!(app(&mut h).s.current_view().size.is_some());
    // the keys did not also move the process selection or the focus
    assert!(app(&mut h).selection.is_none());
    assert!(h.get_by_role_and_label(Role::Splitter, name).accesskit_node().is_focused());
    // Home: the view's own default (30 % of the content height), a long way below 50 %
    press(&mut h, Key::Home);
    assert_eq!(app(&mut h).s.current_view().size, Some(0.30));
    let y4 = splitter(&h, name).center().y;
    assert!(y4 > y3 + 100.0, "back down to the default: {y3} -> {y4}");
    // it never goes past the limits, however long the key is held
    for _ in 0..80 {
        shift_press(&mut h, Key::ArrowUp);
    }
    let top = splitter(&h, name).center().y;
    assert!(top > 100.0, "the sensors keep their room: {top}");
    for _ in 0..80 {
        shift_press(&mut h, Key::ArrowDown);
    }
    assert!(splitter(&h, name).center().y < SIZE.1 - 60.0, "the panel keeps its rows");
}

#[test]
fn the_right_panel_divider_moves_by_arrow_keys_in_pixels_and_home_restores_the_default() {
    let _guard = layout_lock();
    let mut s = dark();
    s.layout.set_dock(Dock::Right);
    let mut h = harness("h-div-right", (1700.0, 900.0), demo_snapshot(), Some(s), false, false);
    let name = "Resize the process panel";
    assert_eq!(app(&mut h).s.current_view().width_px, Some(hwglance::settings::COMPACT_WIDTH_PX));
    focus_splitter(&mut h, name);
    let x0 = splitter(&h, name).center().x;
    // Left moves the divider left, so the panel gets wider
    press(&mut h, Key::ArrowLeft);
    assert_eq!(app(&mut h).s.current_view().width_px, Some(504.0));
    shift_press(&mut h, Key::ArrowLeft);
    assert_eq!(app(&mut h).s.current_view().width_px, Some(536.0));
    let x1 = splitter(&h, name).center().x;
    assert!((x0 - x1 - 36.0).abs() < 1.0, "{x0} -> {x1}");
    shift_press(&mut h, Key::ArrowRight);
    press(&mut h, Key::ArrowRight);
    assert_eq!(app(&mut h).s.current_view().width_px, Some(500.0));
    shift_press(&mut h, Key::ArrowLeft);
    press(&mut h, Key::Home);
    assert_eq!(app(&mut h).s.current_view().width_px, Some(hwglance::settings::COMPACT_WIDTH_PX));
    // the expanded view keeps a share instead, and its divider moves in pixels all the same
    app(&mut h).s.toggle_panel_view();
    h.run_steps(3);
    focus_splitter(&mut h, name);
    let before = app(&mut h).s.current_view().size.unwrap();
    let x0 = splitter(&h, name).center().x;
    shift_press(&mut h, Key::ArrowLeft);
    let after = app(&mut h).s.current_view().size.unwrap();
    assert!(after > before, "{before} -> {after}");
    assert!((x0 - splitter(&h, name).center().x - 32.0).abs() < 1.0);
    press(&mut h, Key::Home);
    assert_eq!(app(&mut h).s.current_view().size, Some(0.55));
}

#[test]
fn the_name_column_divider_moves_by_arrow_keys_and_home_returns_to_the_automatic_width() {
    let _guard = layout_lock();
    let mut s = dark();
    s.processes.views.bottom_expanded.size = Some(0.6);
    s.layout.set_view(hwglance::settings::PanelView::Expanded);
    let mut h = harness("h-div-name", SIZE, demo_snapshot(), Some(s), false, false);
    let name = "Resize the Name column";
    assert!(app(&mut h).s.current_view().name_width.is_none());
    focus_splitter(&mut h, name);
    let x0 = app(&mut h).name_divider_x;
    press(&mut h, Key::ArrowRight);
    let x1 = app(&mut h).name_divider_x;
    assert!((x1 - x0 - 4.0).abs() < 0.6, "wider by 4 px: {x0} -> {x1}");
    assert!(app(&mut h).s.current_view().name_width.is_some());
    shift_press(&mut h, Key::ArrowRight);
    let x2 = app(&mut h).name_divider_x;
    assert!((x2 - x1 - 32.0).abs() < 0.6, "wider by 32 px: {x1} -> {x2}");
    shift_press(&mut h, Key::ArrowLeft);
    press(&mut h, Key::ArrowLeft);
    assert!((app(&mut h).name_divider_x - x0).abs() < 0.6, "and back");
    // held down it stops at the narrowest the column allows, and the focus stays on the divider
    for _ in 0..60 {
        shift_press(&mut h, Key::ArrowLeft);
    }
    let narrow = app(&mut h).name_divider_x;
    assert!(narrow > 100.0, "{narrow}");
    assert!(h.get_by_role_and_label(Role::Splitter, name).accesskit_node().is_focused());
    press(&mut h, Key::Home);
    assert!(app(&mut h).s.current_view().name_width.is_none(), "Home: the automatic width");
    assert!((app(&mut h).name_divider_x - x0).abs() < 0.6);
}

#[test]
fn the_name_divider_of_the_compact_right_panel_moves_the_boundary_and_keeps_the_panel_size() {
    let _guard = layout_lock();
    let mut s = dark();
    s.layout.set_dock(Dock::Right);
    let mut h = harness("h-div-name-right", (1700.0, 900.0), demo_snapshot(), Some(s), false, false);
    let panel_px = app(&mut h).s.current_view().width_px;
    assert!(app(&mut h).s.current_view().data_extra.is_none());
    focus_splitter(&mut h, "Resize the Name column");
    let x0 = app(&mut h).name_divider_x;
    shift_press(&mut h, Key::ArrowLeft);
    assert!((x0 - app(&mut h).name_divider_x - 32.0).abs() < 0.6, "Name is 32 px narrower");
    assert!(app(&mut h).s.current_view().data_extra.is_some(), "the divider sets the data columns' extra width");
    assert_eq!(app(&mut h).s.current_view().width_px, panel_px, "and the panel keeps its size");
    shift_press(&mut h, Key::ArrowRight);
    assert!((app(&mut h).name_divider_x - x0).abs() < 0.6, "and back");
    shift_press(&mut h, Key::ArrowLeft);
    press(&mut h, Key::Home);
    assert!(app(&mut h).s.current_view().data_extra.is_none(), "Home: the fitted widths");
    assert!((app(&mut h).name_divider_x - x0).abs() < 0.6);
    assert_eq!(app(&mut h).s.current_view().width_px, panel_px);
}

#[test]
fn the_dividers_are_tab_stops_that_say_their_size() {
    let _guard = layout_lock();
    let mut h = scene("h-div-tab");
    for name in ["Resize the process panel", "Resize the Name column"] {
        let node = h.get_by_role_and_label(Role::Splitter, name);
        assert!(node.accesskit_node().data().supports_action(egui::accesskit::Action::Focus), "{name} takes the focus");
        assert!(node.accesskit_node().value().is_some_and(|v| v.ends_with("pixels wide") || v.ends_with("pixels high")), "{name} says its size");
    }
    // Tab reaches both
    let mut seen = Vec::new();
    for _ in 0..40 {
        press(&mut h, Key::Tab);
        if let Some(n) = h.root().children_recursive().find(|n| n.accesskit_node().is_focused())
            && n.accesskit_node().role() == Role::Splitter
        {
            seen.push(n.accesskit_node().label().unwrap_or_default());
        }
    }
    assert!(seen.iter().any(|l| l == "Resize the process panel"), "{seen:?}");
    assert!(seen.iter().any(|l| l == "Resize the Name column"), "{seen:?}");
}

// ---------------------------------------------------------------------------------------------
// Edit layout without dragging
// ---------------------------------------------------------------------------------------------

use hwglance::settings::ColumnPref;

/// Two explicit columns that name every card: CPU and Memory in the first, GPU and the rest in the second.
fn two_columns() -> Settings {
    let mut s = dark();
    s.layout.auto = false;
    let rest: Vec<String> = demo_snapshot().sensors.iter().map(|g| g.id.clone()).filter(|id| id != "cpu" && id != "memory").collect();
    s.layout.columns = vec![
        ColumnPref { width: 460.0, cards: vec!["cpu".into(), "memory".into()] },
        ColumnPref { width: 460.0, cards: rest },
    ];
    s
}

fn editing(name: &str) -> H {
    let mut h = harness(name, (1500.0, 3000.0), demo_snapshot(), Some(two_columns()), false, false);
    app(&mut h).edit_layout = true;
    h.run_steps(4);
    h
}

/// The saved columns, with the cards that are not CPU, Memory or GPU left out.
fn columns(h: &mut H) -> Vec<Vec<String>> {
    app(h).s.layout.columns.iter().map(|c| c.cards.iter().filter(|id| ["cpu", "memory", "gpu0"].contains(&id.as_str())).cloned().collect()).collect()
}

fn header_pos(h: &mut H, card: &str) -> egui::Pos2 {
    let r = app(h).card_rects.iter().find(|(id, _)| id == card).unwrap().1;
    egui::pos2(r.center().x + 40.0, r.top() + 18.0)
}

#[test]
fn a_cards_menu_moves_it_to_another_column_and_up_or_down_in_its_own() {
    let _guard = layout_lock();
    let mut h = editing("h-move-card");
    assert_eq!(columns(&mut h), vec![vec!["cpu".to_string(), "memory".to_string()], vec!["gpu0".to_string()]]);
    // right click on the CPU header: the other column and Move down (it is first in its own)
    let p = header_pos(&mut h, "cpu");
    click_secondary(&mut h, p);
    assert!(h.query_by_label("Move to column 2").is_some(), "a column to move to");
    assert!(h.query_by_label("Move to column 1").is_none(), "not its own");
    assert!(h.query_by_label("Move up").is_none(), "first in its column");
    h.get_by_label("Move down").click();
    h.run_steps(4);
    assert_eq!(columns(&mut h), vec![vec!["memory".to_string(), "cpu".to_string()], vec!["gpu0".to_string()]]);
    assert!(!app(&mut h).s.layout.auto, "the arrangement is the user's now");

    // last in its column now: Move up puts it back, and there is no Move down
    let p = header_pos(&mut h, "cpu");
    click_secondary(&mut h, p);
    assert!(h.query_by_label("Move down").is_none(), "last in its column");
    h.get_by_label("Move up").click();
    h.run_steps(4);
    assert_eq!(columns(&mut h), vec![vec!["cpu".to_string(), "memory".to_string()], vec!["gpu0".to_string()]]);

    // to the other column, at its end
    let p = header_pos(&mut h, "cpu");
    click_secondary(&mut h, p);
    h.get_by_label("Move to column 2").click();
    h.run_steps(4);
    assert_eq!(columns(&mut h), vec![vec!["memory".to_string()], vec!["gpu0".to_string(), "cpu".to_string()]]);
    // and the card is drawn there
    let (cpu, gpu) = {
        let a = app(&mut h);
        (a.card_rects.iter().find(|(id, _)| id == "cpu").unwrap().1, a.card_rects.iter().find(|(id, _)| id == "gpu0").unwrap().1)
    };
    assert!(cpu.left() >= gpu.left() - 1.0 && cpu.top() > gpu.top(), "under the GPU card, in column 2");
}

#[test]
fn the_move_entries_are_not_in_the_menu_outside_edit_mode() {
    let _guard = layout_lock();
    let mut h = editing("h-move-card-off");
    app(&mut h).edit_layout = false;
    h.run_steps(4);
    let p = header_pos(&mut h, "cpu");
    click_secondary(&mut h, p);
    assert!(h.query_by_label("Hide panel").is_some(), "the menu is open");
    assert!(h.query_by_label_contains("Move to column").is_none());
}

#[test]
fn the_card_menu_opens_from_the_keyboard_in_edit_mode_with_its_move_entries() {
    let _guard = layout_lock();
    let mut h = editing("h-move-card-key");
    h.query_all_by_label_contains("CPU, AMD").find(|n| !n.accesskit_node().label().unwrap_or_default().starts_with("Move the")).expect("the CPU card header").focus();
    h.run_steps(3);
    shift_press(&mut h, Key::F10);
    assert!(h.query_by_label("Move to column 2").is_some(), "Shift+F10 opens the same menu");
}

#[test]
fn a_section_and_a_row_move_up_and_down_from_their_menus() {
    let _guard = layout_lock();
    let mut h = editing("h-move-rows");
    let snap = app(&mut h).snap.clone().unwrap();
    let cpu = snap.sensors.iter().find(|g| g.id == "cpu").unwrap();
    let first_section = cpu.sections[0].id.clone();
    let second_section = cpu.sections[1].id.clone();
    // right click on the second section's head
    let cpu_rect = app(&mut h).card_rects.iter().find(|(id, _)| id == "cpu").unwrap().1;
    let head = h.query_all_by_role_and_label(Role::Button, &cpu.sections[1].title).find(|n| cpu_rect.contains(n.rect().center())).expect("the section head").rect();
    click_secondary(&mut h, egui::pos2(head.center().x + 40.0, head.center().y));
    h.get_by_label("Move up").click();
    h.run_steps(4);
    let order = app(&mut h).s.layout.section_order.get("cpu").cloned().expect("the section order is saved");
    let at = |id: &str| order.iter().position(|x| x == id).unwrap();
    assert!(at(&second_section) < at(&first_section), "{order:?}");

    // a row: the first row of a section can only move down. Pick a section with several rows
    // and a first row whose label no other row of the card has.
    let all_labels: Vec<&str> = cpu.sections.iter().flat_map(|s| s.readings.iter().map(|r| r.label.as_str())).collect();
    let sec = cpu
        .sections
        .iter()
        .find(|s| s.readings.len() >= 3 && all_labels.iter().filter(|l| **l == s.readings[0].label).count() == 1)
        .expect("a section with several rows");
    let label = format!("CPU, {}, ", sec.readings[0].label);
    let row = h.query_all_by_label_contains(&label).find(|n| cpu_rect.contains(n.rect().center())).expect("a row").rect();
    click_secondary(&mut h, egui::pos2(row.left() + 120.0, row.center().y));
    assert!(h.query_by_label("Watch").is_some(), "the row menu is open");
    assert!(h.query_by_label("Move up").is_none(), "the first row has nothing above it");
    h.get_by_label("Move down").click();
    h.run_steps(4);
    let order = app(&mut h).s.layout.reading_order.get(&sec.id).cloned().expect("the row order is saved");
    assert_eq!(order[1], sec.readings[0].id, "{order:?}");
}

#[test]
fn a_column_edge_resizes_from_the_keyboard() {
    let _guard = layout_lock();
    let mut h = editing("h-col-keys");
    let name = "Resize column 1";
    // the width on screen (a saved layout is scaled to the window; a resize starts from what shows)
    let shown = |h: &mut H| app(h).card_rects.iter().find(|(id, _)| id == "cpu").unwrap().1.width();
    let snap4 = |v: f32| (v / 4.0).round() * 4.0;
    focus_splitter(&mut h, name);
    let w0 = shown(&mut h);
    press(&mut h, Key::ArrowRight);
    assert_eq!(app(&mut h).s.layout.columns[0].width, snap4(w0 + 4.0), "4 px wider than it was on screen");
    let w1 = shown(&mut h);
    shift_press(&mut h, Key::ArrowRight);
    assert_eq!(app(&mut h).s.layout.columns[0].width, snap4(w1 + 32.0), "32 px with Shift");
    let w2 = shown(&mut h);
    press(&mut h, Key::ArrowLeft);
    assert_eq!(app(&mut h).s.layout.columns[0].width, snap4(w2 - 4.0));
    assert!(h.get_by_role_and_label(Role::Splitter, name).accesskit_node().is_focused(), "the focus stays");
    // Home: the width a new column gets
    press(&mut h, Key::Home);
    assert_eq!(app(&mut h).s.layout.columns[0].width, hwglance::layout::NEW_COL_W);
}

// ---------------------------------------------------------------------------------------------
// Extremes: huge lists, hostile names, the smallest windows, the largest and smallest type
// ---------------------------------------------------------------------------------------------

use hwglance_proto::{ProcClass, ProcGroup, Snapshot};

/// The demo snapshot with `n` more process groups, a few of them hostile: a 6 000 character name,
/// a 20 000 character title, Arabic in all three places, a name of nothing but spaces.
fn stress_snapshot(n: usize) -> Snapshot {
    let mut snap = demo_snapshot();
    let proto = snap.procs.as_ref().and_then(|g| g.first()).cloned().expect("demo groups");
    let groups = snap.procs.as_mut().unwrap();
    for i in 0..n {
        let mut g: ProcGroup = proto.clone();
        g.id = format!("app:stress-{i}");
        g.class = [ProcClass::App, ProcClass::Background, ProcClass::System][i % 3];
        g.name = match i {
            0 => "n".repeat(6000),
            1 => "المُفَكِّرَة ".repeat(80),
            2 => "   ".to_string(),
            3 => "emoji \u{1F600}\u{1F468}\u{200D}\u{1F469} and 漢字 and \u{202E}rtl override".to_string(),
            _ => format!("stress process {i}"),
        };
        g.title = match i {
            0 => Some("t".repeat(20_000)),
            1 => Some("عنوان طويل جدا ".repeat(400)),
            _ => (i % 4 == 0).then(|| format!("window {i}")),
        };
        for (k, p) in g.procs.iter_mut().enumerate() {
            p.pid = 100_000 + (i * 4 + k) as i32;
            p.start_time_ticks = 10_000 + (i * 4 + k) as u64;
            p.name = g.name.chars().take(15).collect();
            p.cmdline = if i == 1 { "المفكرة ".repeat(64) } else { "x".repeat(512) };
            p.cpu_pct = (i % 97) as f32 / 10.0;
        }
        g.cpu_pct = g.procs.iter().map(|p| p.cpu_pct).sum();
        groups.push(g);
    }
    snap
}

#[test]
fn a_huge_hostile_process_list_renders_at_every_window_size_font_size_dock_and_view() {
    use hwglance::settings::PanelView;
    let _guard = layout_lock();
    let mut h = harness("h-extremes", (1500.0, 900.0), stress_snapshot(3000), Some(dark()), false, false);
    let total_procs: usize = app(&mut h).groups().iter().map(|g| g.procs.len()).sum();
    assert!(total_procs > 5000, "{total_procs} processes");
    let mut rendered = 0;
    for size in [(800.0, 500.0), (640.0, 360.0), (320.0, 200.0), (160.0, 100.0), (1500.0, 900.0), (2546.0, 1400.0)] {
        for font in [10.0, 14.5, 24.0] {
            for dock in [Dock::Bottom, Dock::Right] {
                for view in [PanelView::Compact, PanelView::Expanded] {
                    h.set_size(egui::vec2(size.0, size.1));
                    {
                        let a = app(&mut h);
                        a.s.appearance.font_size = font;
                        a.s.layout.set_dock(dock);
                        a.s.layout.set_view(view);
                    }
                    h.run_steps(3);
                    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(size.0, size.1));
                    let a = app(&mut h);
                    let ctx = format!("{size:?} font {font} {dock:?} {view:?}");
                    assert!(!a.rows.is_empty(), "{ctx}");
                    // From the smallest size the window allows (800 x 500) up, the gear of the top bar
                    // never leaves the window, however much is in the bar. Below it (a tiling window
                    // manager can force that) nothing may panic, but the bar can run out of room.
                    let gear = a.gear_rect.expect("the gear was drawn");
                    if size.0 >= 800.0 && size.1 >= 500.0 {
                        assert!(screen.contains_rect(gear), "{ctx}: the gear {gear:?} is outside the window");
                    }
                    rendered += 1;
                }
            }
        }
    }
    assert_eq!(rendered, 72);
    // leave the global font size as it was
    app(&mut h).s.appearance.font_size = 14.5;
    h.run_steps(3);
}

#[test]
fn searching_the_huge_list_in_arabic_and_english_stays_quick_and_finds_the_right_rows() {
    let _guard = layout_lock();
    let mut h = harness("h-extremes-search", (1500.0, 900.0), stress_snapshot(3000), Some(dark()), false, false);
    let t = std::time::Instant::now();
    for (q, expect_hit) in [("مفكرة", true), ("المفكره", true), ("stress process 2999", true), ("zzzz-no-such", false), ("", true)] {
        app(&mut h).filter = q.to_string();
        app(&mut h).rows_dirty = true;
        h.run_steps(2);
        let has_groups = app(&mut h).rows.iter().any(|r| matches!(r.kind, hwglance::model::RowKind::Group { .. }));
        assert_eq!(has_groups, expect_hit, "query {q:?}");
    }
    assert!(t.elapsed() < std::time::Duration::from_secs(20), "five searches over 3 000 groups took {:?}", t.elapsed());
    // the empty-result line stays one readable line
    app(&mut h).filter = "q".repeat(500);
    app(&mut h).rows_dirty = true;
    h.run_steps(3);
}

#[test]
fn the_light_and_dark_switch_while_running_keeps_the_stress_list_drawn() {
    use hwglance::theme::Scheme;
    use std::sync::atomic::Ordering;
    let _guard = layout_lock();
    let mut h = harness("h-extremes-theme", (1500.0, 900.0), stress_snapshot(300), None, false, false);
    for (theme, light, scheme) in [("system", true, Scheme::Light), ("system", false, Scheme::Dark), ("light", false, Scheme::Light), ("dark", true, Scheme::Dark)] {
        app(&mut h).s.appearance.theme = theme.into();
        app(&mut h).system_light.store(light, Ordering::Relaxed);
        h.run_steps(3);
        assert_eq!(app(&mut h).pal.scheme, scheme);
        assert!(!app(&mut h).rows.is_empty());
        // a dialog and a menu open across the switch survive it
        app(&mut h).settings_open = true;
        h.run_steps(2);
        app(&mut h).settings_open = false;
        h.run_steps(2);
    }
}

#[test]
fn settings_the_layout_editor_and_the_kill_dialog_survive_the_smallest_windows_and_extreme_type() {
    use hwglance::app::SettingsTab;
    use hwglance::model::RowId;
    let _guard = layout_lock();
    let mut h = harness("h-extremes-dialogs", (1500.0, 900.0), stress_snapshot(200), Some(dark()), false, false);
    let tabs = [
        SettingsTab::General,
        SettingsTab::Sensors,
        SettingsTab::Layout,
        SettingsTab::Processes,
        SettingsTab::History,
        SettingsTab::Alerts,
        SettingsTab::BarWidget,
        SettingsTab::Collector,
    ];
    for size in [(800.0, 500.0), (320.0, 200.0)] {
        for font in [10.0, 24.0] {
            h.set_size(egui::vec2(size.0, size.1));
            app(&mut h).s.appearance.font_size = font;
            h.run_steps(2);
            for tab in tabs {
                let a = app(&mut h);
                a.settings_open = true;
                a.settings_tab = tab;
                h.run_steps(3);
            }
            app(&mut h).settings_open = false;
            app(&mut h).edit_layout = true;
            h.run_steps(3);
            app(&mut h).edit_layout = false;
            // the kill dialog on the 6 000 character name, and on a group of many processes
            let id = RowId::Group("app:stress-0".into());
            {
                let a = app(&mut h);
                a.selection = Some(id.clone());
                a.request_kill(Some(id), hwglance::actions::Signal::Kill);
            }
            h.run_steps(3);
            assert!(app(&mut h).pending_kill.is_some(), "the confirmation is up at {size:?} font {font}");
            app(&mut h).pending_kill = None;
            h.run_steps(2);
        }
    }
    app(&mut h).s.appearance.font_size = 14.5;
    h.run_steps(3);
}

#[test]
fn the_collector_down_banner_wraps_a_long_reason_inside_a_narrow_window() {
    let _guard = layout_lock();
    let mut h = harness("h-banner", (800.0, 500.0), demo_snapshot(), Some(dark()), false, false);
    // a live collector that went away after the first snapshot, with long reasons
    {
        let a = app(&mut h);
        a.mode = hwglance::app::Mode::Live;
        let mut s = a.link.lock();
        s.connected = false;
        s.connect_failed = true;
        s.last_error = Some("The socket exists but nothing answers on it: the service stopped or crashed, and systemd gave up restarting it after the start limit was hit.".into());
        s.action_error = Some("Could not start the collector: Failed to start hwglanced.service: Unit hwglanced.service has a bad unit file setting or is masked by an administrator.".into());
    }
    h.run_steps(4);
    let words = h.query_all_by_label_contains("Collector not running").count();
    assert!(words >= 1, "the banner is up");
    let wide: Vec<_> = h
        .query_all_by_label_contains("the service stopped or crashed")
        .chain(h.query_all_by_label_contains("masked by an administrator"))
        .map(|n| n.rect())
        .filter(|r| r.right() > 800.0 || r.left() < 0.0)
        .collect();
    assert!(wide.is_empty(), "text runs off the window: {wide:?}");
}

#[test]
fn a_focus_request_for_a_reading_that_is_not_reported_says_so_and_one_for_a_hidden_reading_shows_it() {
    let _guard = layout_lock();
    let mut s = dark();
    s.sensors.hidden.push("cpu/temp/tctl".into());
    s.layout.collapsed_cards.push("cpu".into());
    let mut h = harness("h-focus", SIZE, demo_snapshot(), Some(s), false, false);
    let ctx = h.ctx.clone();
    // an id the collector does not report: a toast in words, no pin, nothing scrolls
    app(&mut h).apply_focus(&ctx, "gpu9/temp/nothing", true);
    h.run_steps(2);
    let toast = app(&mut h).toast.as_ref().map(|t| (t.text.clone(), t.ok));
    assert_eq!(toast, Some(("That value is not reported right now".to_string(), false)));
    assert!(app(&mut h).pinned.is_none());
    // a hidden reading in a folded card: both are undone, and the reading is pinned and flashes
    app(&mut h).apply_focus(&ctx, "cpu/temp/tctl", false);
    h.run_steps(2);
    let a = app(&mut h);
    assert!(!a.s.sensors.hidden.contains(&"cpu/temp/tctl".to_string()), "no longer hidden");
    assert!(!a.s.layout.collapsed_cards.contains(&"cpu".to_string()), "the card is open");
    assert_eq!(a.pinned.as_deref(), Some("cpu/temp/tctl"));
    assert!(a.flash.is_some());
    // an alert click while the layout is being edited leaves the editor, whose mode has no popover
    a.edit_layout = true;
    a.pinned = None;
    h.run_steps(2);
    let ctx = h.ctx.clone();
    app(&mut h).apply_focus(&ctx, "cpu/temp/tctl", true);
    assert!(!app(&mut h).edit_layout);
    assert_eq!(app(&mut h).pinned.as_deref(), Some("cpu/temp/tctl"));
}

#[test]
fn a_focus_request_before_the_first_snapshot_waits_for_it() {
    let _guard = layout_lock();
    let mut h = harness("h-focus-early", SIZE, demo_snapshot(), Some(dark()), false, false);
    let ctx = h.ctx.clone();
    app(&mut h).snap = None;
    app(&mut h).apply_focus(&ctx, "cpu/temp/tctl", true);
    assert_eq!(app(&mut h).pending_focus.as_deref(), Some("cpu/temp/tctl"), "kept, not lost");
    assert!(app(&mut h).pinned.is_none());
}

#[test]
fn no_processes_and_one_process_are_drawn_without_trouble() {
    let _guard = layout_lock();
    for n in [0usize, 1] {
        let mut snap = demo_snapshot();
        let groups = snap.procs.as_mut().unwrap();
        groups.truncate(n);
        if let Some(g) = groups.first_mut() {
            g.procs.truncate(1);
        }
        let mut h = harness(&format!("h-procs-{n}"), (800.0, 500.0), snap, Some(dark()), false, false);
        app(&mut h).selection = app(&mut h).rows.iter().find(|r| matches!(r.id, hwglance::model::RowId::Group(_))).map(|r| r.id.clone());
        h.run_steps(3);
        // a search that finds nothing says so
        app(&mut h).filter = "zzz".into();
        app(&mut h).rows_dirty = true;
        h.run_steps(3);
        assert!(h.query_by_label_contains("No process matches").is_some(), "n = {n}");
        assert!(app(&mut h).groups().len() == n);
    }
}
