//! Batch D in the window: accessible names, a usable keyboard path (focus ring, one Tab stop per
//! card, the menu key), 24 px hit areas, the history popover's label layout and the status bar at
//! a narrow width. Writes PNGs into `HWGLANCE_SHOTS` when it is set.

mod common;

use common::*;
use egui::accesskit::{Action, Role};
use egui::{pos2, Key, Modifiers};
use egui_kittest::kittest::{NodeT, Queryable};
use hwglance::settings::Settings;

const BIG: (f32, f32) = (2546.0, 1400.0);

fn dark() -> Settings {
    let mut s = Settings::default();
    s.appearance.theme = "dark".into();
    s
}

fn light() -> Settings {
    let mut s = Settings::default();
    s.appearance.theme = "light".into();
    s
}

fn scene(name: &str, size: (f32, f32), s: Settings) -> H {
    harness(name, size, demo_snapshot(), Some(s), false, false)
}

/// The focused node: (role, label, rect).
fn focused(h: &H) -> Option<(Role, String, egui::Rect)> {
    h.root().children_recursive().find(|n| n.accesskit_node().is_focused()).map(|n| {
        let ak = n.accesskit_node();
        (ak.role(), ak.label().unwrap_or_default(), n.rect())
    })
}

fn press(h: &mut H, key: Key) {
    h.key_press(key);
    h.run_steps(3);
}

fn shift(h: &mut H, key: Key) {
    h.key_press_modifiers(Modifiers::SHIFT, key);
    h.run_steps(3);
}

/// Tab through the window until the focus wraps; the stops in order.
fn tab_stops(h: &mut H, max: usize) -> Vec<(Role, String, egui::Rect)> {
    let mut out: Vec<(Role, String, egui::Rect)> = Vec::new();
    for _ in 0..max {
        press(h, Key::Tab);
        let Some(f) = focused(h) else { continue };
        if out.first().is_some_and(|first| first.1 == f.1 && first.2 == f.2) {
            break;
        }
        out.push(f);
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Accessible names
// ---------------------------------------------------------------------------------------------

/// Interactive nodes (they take a click or the focus) with neither a name nor a value.
fn unnamed_interactive(h: &H) -> Vec<(Role, egui::Rect)> {
    h.root()
        .children_recursive()
        .filter(|n| {
            let ak = n.accesskit_node();
            (ak.data().supports_action(Action::Click) || ak.data().supports_action(Action::Focus))
                && ak.label().is_none_or(|l| l.is_empty())
                && ak.value().is_none_or(|v| v.is_empty())
                && !ak.is_hidden()
                // egui's own scroll bars are what their role says
                && ak.role() != Role::ScrollBar
        })
        .map(|n| (n.accesskit_node().role(), n.rect()))
        .collect()
}

#[test]
fn every_interactive_node_of_the_window_has_a_name() {
    let _guard = layout_lock();
    let mut h = scene("d-names", BIG, dark());
    let bad = unnamed_interactive(&h);
    assert!(bad.is_empty(), "{} unnamed interactive nodes: {bad:?}", bad.len());
    // also with the process panel expanded, the layout editor and Settings open
    app(&mut h).edit_layout = true;
    h.run_steps(3);
    let bad = unnamed_interactive(&h);
    assert!(bad.is_empty(), "edit layout: {} unnamed: {bad:?}", bad.len());
    app(&mut h).edit_layout = false;
    app(&mut h).settings_open = true;
    app(&mut h).settings_tab = hwglance::app::SettingsTab::BarWidget;
    h.run_steps(4);
    let bad = unnamed_interactive(&h);
    assert!(bad.is_empty(), "Settings, Bar widget: {} unnamed: {bad:?}", bad.len());
}

#[test]
fn painted_controls_say_what_they_are_and_what_state_they_are_in() {
    let _guard = layout_lock();
    let h = scene("d-names2", BIG, dark());
    // a card header: a button named by device and model, open
    let head = h.get_by_label_contains("CPU, AMD Ryzen");
    assert_eq!(head.accesskit_node().role(), Role::Button);
    assert_eq!(head.accesskit_node().data().is_expanded(), Some(true));
    // a section head is a button that says whether it is open
    let temps = h.query_all_by_role_and_label(Role::Button, "Temperatures").next().expect("a section head");
    assert_eq!(temps.accesskit_node().data().is_expanded(), Some(true));
    // a reading row: device, label, the value in words, and the limits
    let row = h.get_by_label_contains("CPU, Tctl, ");
    let name = row.accesskit_node().label().unwrap();
    assert_eq!(row.accesskit_node().role(), Role::ListItem);
    assert!(name.contains(" degrees"), "{name}");
    assert!(name.contains("warning above"), "{name}");
    // a process row: who, how many, how busy and how big, selected or not
    let proc_row = h.query_all_by_role(Role::ListItem).find(|n| n.accesskit_node().label().is_some_and(|l| l.contains("processes, CPU "))).expect("a group row");
    let name = proc_row.accesskit_node().label().unwrap();
    assert!(name.contains("memory "), "{name}");
    assert_eq!(proc_row.accesskit_node().data().is_selected(), Some(false));
    // glyph-only buttons are named by what they do, never by the private-use glyph
    for name in ["Settings", "Expand the process panel", "Dock the process panel at the right", "Dock the process panel below the sensors"] {
        assert!(h.query_by_role_and_label(Role::Button, name).is_some(), "{name}");
    }
    let private_use = |s: &str| s.chars().any(|c| (c as u32) >= 0xF0000);
    let glyph_named: Vec<_> = h.root().children_recursive().filter(|n| n.accesskit_node().label().is_some_and(|l| private_use(&l))).collect();
    assert!(glyph_named.is_empty(), "{} nodes are named by a glyph", glyph_named.len());
    // dividers are splitters with a name
    assert!(h.query_by_role_and_label(Role::Splitter, "Resize the process panel").is_some());
    assert!(h.query_by_role_and_label(Role::Splitter, "Resize the Name column").is_some());
    // a column header says which way it is sorted
    let cpu = h.query_all_by_role(Role::ColumnHeader).find(|n| n.accesskit_node().label() == Some("CPU".into())).expect("the CPU header");
    assert!(cpu.accesskit_node().data().sort_direction().is_some(), "the sorted column says so");
    // the Sources chip says how many sources answer
    assert!(h.query_by_label_contains("Sources, 4 of 4 answering").is_some());
}

// ---------------------------------------------------------------------------------------------
// Keyboard
// ---------------------------------------------------------------------------------------------

#[test]
fn tab_visits_one_stop_per_card_and_reads_left_to_right() {
    let _guard = layout_lock();
    let mut h = scene("d-tab", BIG, dark());
    let stops = tab_stops(&mut h, 80);
    // 148 stops before; now the top bar, one per card, the process panel's controls, its header and its list
    assert!(stops.len() <= 30, "{} stops: {:?}", stops.len(), stops.iter().map(|s| s.1.as_str()).collect::<Vec<_>>());
    let at = |name: &str| stops.iter().position(|s| s.1.starts_with(name)).unwrap_or_else(|| panic!("no stop {name}"));
    // the top bar, left to right: the legend, then Reset, Sources, Edit layout, Settings, Maximize and Close
    let bar: Vec<&(Role, String, egui::Rect)> = stops.iter().take_while(|s| s.2.top() < 60.0).collect();
    assert!(bar.len() >= 9, "{} top bar stops", bar.len());
    assert!(bar.windows(2).all(|w| w[0].2.left() < w[1].2.left()), "the top bar is not left to right: {:?}", bar.iter().map(|s| (&s.1, s.2.left())).collect::<Vec<_>>());
    assert!(at("Reset") < at("Sources") && at("Sources") < at("Edit layout") && at("Edit layout") < at("Settings"));
    assert!(at("Settings") < at("Maximize") && at("Maximize") < at("Close") && at("Close") < at("CPU, AMD"));
    // then one stop per card header (a row is never a stop), then the process panel
    let cards = app(&mut h).card_rects.len();
    let heads = stops.iter().filter(|s| s.2.height() == 36.0 && s.0 == Role::Button).count();
    assert_eq!(heads, cards, "one stop for each of the {cards} cards");
    assert!(stops.iter().all(|s| s.0 != Role::ListItem), "no row is a stop");
    assert!(at("Settings") < at("CPU, AMD") && at("Board, ") < at("Search processes"));
    assert!(at("Search processes") < at("Expand the process panel") && at("Expand the process panel") < at("Hide the process panel"));
    assert!(at("Hide the process panel") < at("Sort the process table") && at("Sort the process table") < at("Processes"));
    // the dividers are stops since they have a keyboard path: the Name divider follows the header, and
    // the panel's own divider comes after the list, which it was made after (it must win the pixels)
    let n = stops.len();
    assert_eq!((stops[n - 1].0, stops[n - 1].1.as_str()), (Role::Splitter, "Resize the process panel"), "the panel divider is the last stop");
    assert_eq!(stops[n - 2].0, Role::List, "the process list is the one before");
    assert_eq!((stops[n - 3].0, stops[n - 3].1.as_str()), (Role::Splitter, "Resize the Name column"));
    assert!(stops[n - 4].1.starts_with("Sort the process table"));
    // the walk wrapped to the first stop; Shift+Tab goes to the last and walks back the same way
    shift(&mut h, Key::Tab);
    assert_eq!(focused(&h).map(|f| f.0), Some(Role::Splitter));
    shift(&mut h, Key::Tab);
    assert_eq!(focused(&h).map(|f| f.0), Some(Role::List));
    shift(&mut h, Key::Tab);
    shift(&mut h, Key::Tab);
    assert!(focused(&h).unwrap().1.starts_with("Sort the process table"), "back from the list, past the Name divider, onto the table header");
}

#[test]
fn the_arrow_keys_walk_a_card_and_tab_remembers_where_they_left_it() {
    let _guard = layout_lock();
    let mut h = scene("d-arrows", BIG, dark());
    // onto the CPU card header: twelve stops in the top bar, then the card
    for _ in 0..13 {
        press(&mut h, Key::Tab);
    }
    let (_, head, _) = focused(&h).unwrap();
    assert!(head.starts_with("CPU, AMD"), "{head}");
    // Down: the first section, then its first row
    press(&mut h, Key::ArrowDown);
    let (role, sec, _) = focused(&h).unwrap();
    assert_eq!((role, sec.as_str()), (Role::Button, "Temperatures"));
    press(&mut h, Key::ArrowDown);
    let (role, row, rect) = focused(&h).unwrap();
    assert_eq!(role, Role::ListItem);
    assert!(row.starts_with("CPU, "), "{row}");
    assert!(app(&mut h).selection.is_none(), "walking a card never moves the process selection");
    // Enter pins the row's history, like a click
    press(&mut h, Key::Enter);
    assert!(app(&mut h).pinned.is_some(), "Enter pins the popover");
    press(&mut h, Key::Escape);
    assert!(app(&mut h).pinned.is_none(), "Esc lets go of it");
    assert_eq!(focused(&h).map(|f| f.1), Some(row.clone()), "and keeps the row's focus");
    // Tab goes to the next card, Shift+Tab comes back to the same row (the stop moved with the arrows)
    press(&mut h, Key::Tab);
    assert!(focused(&h).unwrap().1.starts_with("GPU, "));
    shift(&mut h, Key::Tab);
    let (_, back, back_rect) = focused(&h).unwrap();
    assert_eq!((back, back_rect), (row, rect));
    // Up twice is the header again; End is the last row of the card; Home is the header
    press(&mut h, Key::ArrowUp);
    press(&mut h, Key::ArrowUp);
    assert!(focused(&h).unwrap().1.starts_with("CPU, AMD"));
    press(&mut h, Key::End);
    assert_eq!(focused(&h).unwrap().0, Role::Button, "the last stop of the CPU card is a collapsed section head or a row");
    press(&mut h, Key::Home);
    assert!(focused(&h).unwrap().1.starts_with("CPU, AMD"));
    // Left collapses the card, Right opens it again
    press(&mut h, Key::ArrowLeft);
    assert!(app(&mut h).s.layout.collapsed_cards.contains(&"cpu".to_string()));
    press(&mut h, Key::ArrowRight);
    assert!(!app(&mut h).s.layout.collapsed_cards.contains(&"cpu".to_string()));
}

#[test]
fn the_process_table_is_one_stop_where_up_and_down_move_the_selection() {
    let _guard = layout_lock();
    let mut h = scene("d-list", BIG, dark());
    let stops = tab_stops(&mut h, 80);
    assert_eq!(stops[stops.len() - 2].0, Role::List, "the list, then the panel divider");
    shift(&mut h, Key::Tab);
    shift(&mut h, Key::Tab);
    assert_eq!(focused(&h).map(|f| f.0), Some(Role::List));
    assert!(app(&mut h).selection.is_some(), "arriving on the list selects its first row");
    let first = app(&mut h).selection.clone();
    press(&mut h, Key::ArrowDown);
    assert_ne!(app(&mut h).selection, first);
    assert_eq!(focused(&h).map(|f| f.0), Some(Role::List), "the focus stays on the list");
    // the header is a stop too (past the Name divider): Right chooses a column, Enter sorts by it
    shift(&mut h, Key::Tab);
    assert_eq!(focused(&h).map(|f| f.0), Some(Role::Splitter), "the Name divider");
    shift(&mut h, Key::Tab);
    assert!(focused(&h).unwrap().1.starts_with("Sort the process table by Name"));
    press(&mut h, Key::ArrowRight);
    assert!(focused(&h).unwrap().1.ends_with("PID"), "{:?}", focused(&h));
    press(&mut h, Key::Enter);
    assert_eq!(app(&mut h).sort_key(), hwglance::model::SortKey::Pid);
}

#[test]
fn shift_f10_opens_the_menu_of_the_focused_row_and_returns_the_focus() {
    let _guard = layout_lock();
    let mut h = scene("d-menu", BIG, dark());
    // nothing focused: nothing opens
    shift(&mut h, Key::F10);
    assert!(h.query_by_label("Watch").is_none());
    // a reading row: the value menu
    for _ in 0..13 {
        press(&mut h, Key::Tab);
    }
    press(&mut h, Key::ArrowDown);
    press(&mut h, Key::ArrowDown);
    let (_, row, _) = focused(&h).unwrap();
    shift(&mut h, Key::F10);
    assert!(h.query_by_label("Watch").is_some(), "the value menu is open");
    assert!(h.query_by_label("Copy value").is_some());
    press(&mut h, Key::Escape);
    assert!(h.query_by_label("Watch").is_none(), "Esc closes it");
    assert_eq!(focused(&h).map(|f| f.1), Some(row), "and the row has the focus again");
    // a section head: its menu
    press(&mut h, Key::ArrowUp);
    shift(&mut h, Key::F10);
    assert!(h.query_by_label("Hide section").is_some());
    press(&mut h, Key::Escape);
    // a card header: its menu
    press(&mut h, Key::Home);
    shift(&mut h, Key::F10);
    assert!(h.query_by_label("Hide panel").is_some());
    press(&mut h, Key::Escape);
    // the process list: the menu of the selected process (a group row, not a class row)
    app(&mut h).move_selection(1);
    app(&mut h).move_selection(1);
    h.ctx.memory_mut(|m| m.request_focus(egui::Id::new(hwglance::procs::LIST_ID)));
    h.run_steps(3);
    shift(&mut h, Key::F10);
    assert!(h.query_by_label("Copy PID").is_some() && h.query_by_label("Copy command line").is_some(), "the process menu");
    press(&mut h, Key::Escape);
    assert!(h.query_by_label("Copy PID").is_none());
    assert_eq!(focused(&h).map(|f| f.0), Some(Role::List), "the list has the focus again");
}

#[test]
fn the_focus_ring_is_two_pixels_of_ink_one_pixel_off_in_both_palettes() {
    let _guard = layout_lock();
    for (name, settings, pal) in [("dark", dark(), hwglance::theme::Palette::dark()), ("light", light(), hwglance::theme::Palette::light())] {
        let mut h = harness(&format!("d-ring-{name}"), BIG, demo_snapshot(), Some(settings), true, false);
        let ink = pal.ink;
        let is_ink = |img: &image::RgbaImage, x: f32, y: f32| {
            let p = img.get_pixel(x as u32, y as u32).0;
            (0..3).all(|i| (p[i] as i32 - [ink.r(), ink.g(), ink.b()][i] as i32).abs() <= 8)
        };
        // before any key the chip has no ring
        let before = h.render().expect("render");
        let chip = h.get_by_label_contains("Jump to the CPU panel").rect();
        assert!(!is_ink(&before, chip.center().x, chip.top() - 2.0), "{name}: no ring without focus");
        // a legend chip wears the ring outside: its edge is 1 px off, the ring 2 px wide
        press(&mut h, Key::Tab);
        assert!(focused(&h).unwrap().1.contains("Jump to the CPU panel"));
        let img = h.render().expect("render");
        let x = chip.center().x;
        assert!(is_ink(&img, x, chip.top() - 2.5) && is_ink(&img, x, chip.top() - 1.5), "{name}: the ring above the chip");
        assert!(!is_ink(&img, x, chip.top() - 0.5), "{name}: a gap of 1 px");
        assert!(!is_ink(&img, x, chip.top() - 3.5), "{name}: and no thicker than 2 px");
        assert!(is_ink(&img, x, chip.bottom() + 1.5) && is_ink(&img, chip.left() - 1.5, chip.center().y), "{name}: all round");
        // a card header wears it inside, against its edge
        for _ in 0..12 {
            press(&mut h, Key::Tab);
        }
        let (_, label, head) = focused(&h).unwrap();
        assert!(label.starts_with("CPU, AMD"), "{label}");
        let img = h.render().expect("render");
        let x = head.center().x;
        assert!(is_ink(&img, x, head.top() + 1.5) && is_ink(&img, x, head.top() + 2.5), "{name}: the ring inside the header");
        assert!(!is_ink(&img, x, head.top() + 0.5) && !is_ink(&img, x, head.top() + 3.5));
        // a reading row (after two arrows) and the process list (its selected row) wear it too
        press(&mut h, Key::ArrowDown);
        press(&mut h, Key::ArrowDown);
        let (_, _, row) = focused(&h).unwrap();
        let img = h.render().expect("render");
        assert!(is_ink(&img, row.center().x, row.top() + 1.5), "{name}: the ring on a row");
        // the process list: the ring is on its selected row, here the first
        app(&mut h).move_selection(1);
        h.ctx.memory_mut(|m| m.request_focus(egui::Id::new(hwglance::procs::LIST_ID)));
        h.run_steps(3);
        let list = app(&mut h).procs_list_rect;
        let img = h.render().expect("render");
        assert!(is_ink(&img, list.left() + 600.0, list.top() + 1.5), "{name}: the ring on the selected process row");
    }
}

#[test]
fn every_interactive_hit_area_is_at_least_24_px_in_its_small_dimension() {
    let _guard = layout_lock();
    let interactive = |role: Role| {
        matches!(
            role,
            Role::Button | Role::CheckBox | Role::RadioButton | Role::Slider | Role::SpinButton | Role::TextInput | Role::SearchInput | Role::ListItem | Role::ColumnHeader | Role::Splitter | Role::ComboBox | Role::MenuItem | Role::Tab | Role::Link | Role::List
        )
    };
    let check = |h: &H, what: &str| {
        let mut small = Vec::new();
        for n in h.root().children_recursive() {
            let ak = n.accesskit_node();
            // egui's own scroll bars are 8 px bars with keyboard and wheel alternatives
            if !interactive(ak.role()) || ak.is_hidden() {
                continue;
            }
            let r = n.rect();
            if r.width() < 23.99 || r.height() < 23.99 {
                small.push(format!("{:?} {:?} {:.0}x{:.0}", ak.role(), ak.label().unwrap_or_default(), r.width(), r.height()));
            }
        }
        assert!(small.is_empty(), "{what}: {} hit areas under 24 px: {small:?}", small.len());
    };
    let mut h = scene("d-hit", BIG, dark());
    check(&h, "default");
    // the layout editor: grips, Remove column and the column resize handles
    app(&mut h).edit_layout = true;
    h.run_steps(3);
    check(&h, "edit layout");
    let grips = h.query_all_by_label_contains("Move ").count();
    assert!(grips > 50, "{grips} grips");
    app(&mut h).edit_layout = false;
    // both docks and both views
    for (dock, view) in [("right", "compact"), ("right", "expanded"), ("bottom", "expanded")] {
        let mut s = dark();
        s.layout.process_dock = dock.into();
        s.layout.process_view.right = view.into();
        s.layout.process_view.bottom = view.into();
        let h = scene("d-hit-dock", BIG, s);
        check(&h, &format!("{dock} {view}"));
    }
    // Settings, every tab (the Bar widget tab has the 29 x 19 list buttons, the window the edge grabs)
    app(&mut h).settings_open = true;
    for tab in [
        hwglance::app::SettingsTab::General,
        hwglance::app::SettingsTab::Sensors,
        hwglance::app::SettingsTab::Layout,
        hwglance::app::SettingsTab::Processes,
        hwglance::app::SettingsTab::History,
        hwglance::app::SettingsTab::Alerts,
        hwglance::app::SettingsTab::BarWidget,
        hwglance::app::SettingsTab::Collector,
    ] {
        app(&mut h).settings_tab = tab;
        h.run_steps(4);
        check(&h, &format!("Settings {tab:?}"));
    }
    // the smallest type: the floor holds where `snap4(24 x scale)` would not
    app(&mut h).settings_open = false;
    app(&mut h).s.appearance.font_size = 10.0;
    h.run_steps(6);
    check(&h, "10 px type");
    app(&mut h).edit_layout = true;
    h.run_steps(4);
    check(&h, "10 px type, edit layout");
    app(&mut h).s.appearance.font_size = 14.5;
    h.run_steps(4);
}

/// The labels of the status bar (the bottom strip), left to right: text and rectangle.
fn status_labels(h: &H, height: f32) -> Vec<(String, egui::Rect)> {
    let mut v: Vec<(String, egui::Rect)> = h
        .root()
        .children_recursive()
        .filter(|n| matches!(n.accesskit_node().role(), Role::Label))
        .filter(|n| n.rect().top() > height - 40.0 && n.rect().width() > 0.0)
        .map(|n| (n.accesskit_node().value().unwrap_or_default(), n.rect()))
        .collect();
    v.sort_by(|a, b| a.1.left().total_cmp(&b.1.left()));
    v
}

#[test]
fn the_status_bar_compacts_below_800_px_instead_of_overlapping() {
    let _guard = layout_lock();
    let mut previous_len = usize::MAX;
    for w in [1600.0, 1100.0, 900.0, 800.0, 760.0, 640.0, 520.0] {
        let mut h = scene("d-status", (w, 700.0), dark());
        // a toast on the right too, the widest the right end gets
        app(&mut h).set_toast("Min, max and average reset".into(), true);
        h.run_steps(3);
        let labels = status_labels(&h, 700.0);
        assert!(labels.iter().any(|l| l.0.starts_with("updated")), "{w}: the age of the data is shown: {labels:?}");
        for pair in labels.windows(2) {
            assert!(pair[0].1.right() <= pair[1].1.left() + 0.5, "{w}: {:?} runs into {:?}", pair[0], pair[1]);
        }
        let left_len: usize = labels.iter().filter(|l| !l.0.starts_with("updated") && !l.0.starts_with("Min, max")).map(|l| l.0.len()).sum();
        assert!(left_len <= previous_len, "{w}: the left end only ever gets shorter as the window narrows");
        previous_len = left_len;
        assert!(labels.last().unwrap().1.right() <= w, "{w}: nothing leaves the window");
        if w >= 1100.0 {
            assert!(labels.iter().any(|l| l.0.contains(" / ") && l.0.contains("GiB")), "{w}: the full memory text: {labels:?}");
        }
        if w <= 760.0 {
            assert!(!labels.iter().any(|l| l.0.contains(" / ")), "{w}: the memory total is dropped: {labels:?}");
            assert!(labels.iter().any(|l| l.0 == "Memory"), "{w}: but Memory is still said");
        }
    }
}

/// The batch B popover scene: the fixture's Tctl past its warning level, a history that humps up
/// to its peak, the popover pinned open.
fn popover_scene(name: &str, settings: Settings, render: bool) -> H {
    use hwglance_proto::{HistorySeries, Stats, Unit};
    let mut snap = repo_fixture();
    snap.history_minutes = 30;
    let now = snap.unix_ms;
    let r = snap.sensors.iter_mut().flat_map(|g| g.sections.iter_mut()).flat_map(|x| x.readings.iter_mut()).find(|r| r.id == "cpu/temp/tctl").unwrap();
    r.value = Some(83.7);
    r.stats = Some(Stats { min: 58.0, max: 84.2, avg: 62.0, samples: 600 });
    let mut h = harness(name, BIG, snap, Some(settings), render, false);
    let ctx = h.ctx.clone();
    let points: Vec<(u64, f32)> = (0..360)
        .map(|i| {
            let t = now - 1_800_000 + i as u64 * 5000;
            let hump = if (250..340).contains(&i) { 22.0 * (1.0 - ((i as f32 - 295.0) / 45.0).powi(2)).max(0.0) } else { 0.0 };
            (t, 58.0 + hump + ((i * 7) % 5) as f32 * 0.4)
        })
        .collect();
    app(&mut h).link.publish_history(&ctx, vec![HistorySeries { id: "cpu/temp/tctl".into(), unit: Unit::Celsius, points }]);
    app(&mut h).pinned = Some("cpu/temp/tctl".into());
    h.hover_at(pos2(5.0, 5.0));
    h.run_steps(5);
    h
}

#[test]
fn the_history_popover_has_a_text_equivalent() {
    let _guard = layout_lock();
    let h = popover_scene("d-pop-text", dark(), false);
    let node = h.query_all_by_role(Role::Image).find(|n| n.accesskit_node().label().is_some_and(|l| l.starts_with("History of CPU, Tctl"))).expect("the plot's node");
    let text = node.accesskit_node().label().unwrap();
    assert!(text.contains("lowest 58.") && text.contains("highest 8"), "{text}");
}

/// Writes the renders of this batch (`HWGLANCE_SHOTS=<dir>`): focused states, the popover and the
/// narrow status bar, dark and light.
#[test]
fn renders_of_the_batch() {
    let Some(dir) = shots_dir() else { return };
    let _guard = layout_lock();
    std::fs::create_dir_all(&dir).unwrap();
    for (name, settings) in [("dark", dark()), ("light", light())] {
        // the popover with a threshold label and the maximum near the top
        let mut h = popover_scene(&format!("d-r-pop-{name}"), settings.clone(), true);
        save(&mut h, &format!("d1-popover-{name}.png"));
        // focus: a legend chip, a card header, a reading row, the process table's header and list
        let mut h = harness(&format!("d-r-focus-{name}"), BIG, demo_snapshot(), Some(settings.clone()), true, false);
        press(&mut h, Key::Tab);
        save(&mut h, &format!("d2-focus-chip-{name}.png"));
        for _ in 0..10 {
            press(&mut h, Key::Tab);
        }
        save(&mut h, &format!("d3-focus-card-{name}.png"));
        press(&mut h, Key::ArrowDown);
        press(&mut h, Key::ArrowDown);
        press(&mut h, Key::ArrowDown);
        save(&mut h, &format!("d4-focus-row-{name}.png"));
        shift(&mut h, Key::F10);
        save(&mut h, &format!("d5-row-menu-by-key-{name}.png"));
        press(&mut h, Key::Escape);
        app(&mut h).move_selection(1);
        app(&mut h).move_selection(1);
        h.ctx.memory_mut(|m| m.request_focus(egui::Id::new(hwglance::procs::LIST_ID)));
        h.run_steps(3);
        save(&mut h, &format!("d6-focus-process-list-{name}.png"));
        h.ctx.memory_mut(|m| m.request_focus(egui::Id::new(hwglance::procs::HEADER_ID)));
        h.run_steps(3);
        press(&mut h, Key::ArrowRight);
        press(&mut h, Key::ArrowRight);
        save(&mut h, &format!("d7-focus-table-header-{name}.png"));
        // the status bar below 800 px
        let mut h = harness(&format!("d-r-status-{name}"), (760.0, 700.0), demo_snapshot(), Some(settings.clone()), true, false);
        save(&mut h, &format!("d8-status-bar-760-{name}.png"));
        // the layout editor with its 24 px grips
        let mut h = harness(&format!("d-r-edit-{name}"), BIG, demo_snapshot(), Some(settings), true, false);
        app(&mut h).edit_layout = true;
        h.run_steps(3);
        save(&mut h, &format!("d9-edit-layout-{name}.png"));
    }
}

#[test]
fn escape_still_closes_settings() {
    let _guard = layout_lock();
    let mut h = scene("d-esc", BIG, dark());
    app(&mut h).settings_open = true;
    h.run_steps(3);
    assert!(h.query_by_label("Close settings").is_some());
    press(&mut h, Key::Escape);
    assert!(!app(&mut h).settings_open);
}
