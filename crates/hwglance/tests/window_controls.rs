//! The maximize and close buttons at the right end of the top bar: their names, their place in
//! the Tab order, their size, and the double-click on the bar.

mod common;

use common::*;
use egui::accesskit::Role;
use egui::{pos2, Key};
use egui_kittest::kittest::{NodeT, Queryable};
use hwglance::settings::Settings;

fn dark() -> Settings {
    let mut s = Settings::default();
    s.appearance.theme = "dark".into();
    s
}

fn scene(name: &str, size: (f32, f32)) -> H {
    harness(name, size, demo_snapshot(), Some(dark()), false, false)
}

fn set_maximized(h: &mut H, on: bool) {
    h.input_mut().viewports.get_mut(&egui::ViewportId::ROOT).unwrap().maximized = Some(on);
    h.run_steps(2);
}

/// The viewport commands of the next `n` frames (the harness keeps only the last frame's).
fn commands(h: &mut H, n: usize) -> Vec<egui::ViewportCommand> {
    let mut all = Vec::new();
    for _ in 0..n {
        h.step();
        all.extend(h.output().viewport_output.get(&egui::ViewportId::ROOT).map(|v| v.commands.clone()).unwrap_or_default());
    }
    all
}

fn focused_label(h: &H) -> Option<String> {
    h.root().children_recursive().find(|n| n.accesskit_node().is_focused()).and_then(|n| n.accesskit_node().label())
}

#[test]
fn the_top_bar_has_maximize_and_close_buttons_that_are_named_and_sized() {
    let _guard = layout_lock();
    let h = scene("wc-names", (1600.0, 900.0));
    let max = h.get_by_role_and_label(Role::Button, "Maximize").rect();
    let close = h.get_by_role_and_label(Role::Button, "Close").rect();
    let gear = h.get_by_role_and_label(Role::Button, "Settings").rect();
    assert!(gear.right() < max.left() && max.right() < close.left(), "left to right: gear, maximize, close");
    assert!(max.left() - gear.right() >= 8.0, "a gap separates the group from the gear");
    for r in [max, close] {
        assert!(r.width() >= 24.0 && r.height() >= 24.0, "{r:?}");
        assert!((r.height() - gear.height()).abs() < 0.5, "the gear's height");
    }
    assert!(close.right() <= 1600.0 - 8.0, "inside the window");
    // no minimize: the compositor ignores it
    assert!(h.query_by_role_and_label(Role::Button, "Minimize").is_none());
}

#[test]
fn maximize_becomes_restore_while_the_window_is_maximized() {
    let _guard = layout_lock();
    let mut h = scene("wc-restore", (1600.0, 900.0));
    set_maximized(&mut h, true);
    assert!(h.query_by_role_and_label(Role::Button, "Restore").is_some());
    assert!(h.query_by_role_and_label(Role::Button, "Maximize").is_none());
    set_maximized(&mut h, false);
    assert!(h.query_by_role_and_label(Role::Button, "Maximize").is_some());
}

#[test]
fn the_buttons_send_the_viewport_commands() {
    let _guard = layout_lock();
    let mut h = scene("wc-cmds", (1600.0, 900.0));
    h.get_by_role_and_label(Role::Button, "Maximize").click();
    assert!(commands(&mut h, 3).iter().any(|c| matches!(c, egui::ViewportCommand::Maximized(true))));
    set_maximized(&mut h, true);
    h.get_by_role_and_label(Role::Button, "Restore").click();
    assert!(commands(&mut h, 3).iter().any(|c| matches!(c, egui::ViewportCommand::Maximized(false))));
    h.get_by_role_and_label(Role::Button, "Close").click();
    assert!(commands(&mut h, 3).iter().any(|c| matches!(c, egui::ViewportCommand::Close)));
}

#[test]
fn tab_goes_from_settings_to_maximize_to_close() {
    let _guard = layout_lock();
    let mut h = scene("wc-tab", (1600.0, 900.0));
    h.get_by_role_and_label(Role::Button, "Settings").focus();
    h.run_steps(2);
    for want in ["Maximize", "Close"] {
        h.key_press(Key::Tab);
        h.run_steps(3);
        assert_eq!(focused_label(&h).as_deref(), Some(want));
    }
}

#[test]
fn a_double_click_on_an_empty_part_of_the_bar_toggles_maximize_and_nothing_else_does() {
    let _guard = layout_lock();
    let mut h = harness_with_dt("wc-dbl", (1600.0, 900.0), demo_snapshot(), Some(dark()), 0.05);
    let bar_y = h.get_by_role_and_label(Role::Button, "Settings").rect().center().y;
    // clicks `times` times at `at` and returns the commands sent meanwhile
    let click = |h: &mut H, at: egui::Pos2, times: usize| {
        let mut sent = Vec::new();
        h.event(egui::Event::PointerMoved(at));
        sent.extend(commands(h, 1));
        for _ in 0..times {
            for pressed in [true, false] {
                h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE });
                sent.extend(commands(h, 1));
            }
        }
        sent.extend(commands(h, 2));
        sent
    };
    let maximizes = |v: &[egui::ViewportCommand]| v.iter().any(|c| matches!(c, egui::ViewportCommand::Maximized(_)));
    // empty space: left of the right-hand buttons, right of the legend
    let empty = pos2(800.0, bar_y);
    assert!(maximizes(&click(&mut h, empty, 2)), "double-click on the bar");
    assert!(!maximizes(&click(&mut h, empty, 1)), "one click does nothing");
    // a double-click on a button is the button's, not the bar's
    let gear = h.get_by_role_and_label(Role::Button, "Settings").rect().center();
    assert!(!maximizes(&click(&mut h, gear, 2)));
}

#[test]
fn at_1200_by_700_the_bar_still_fits_with_both_buttons() {
    let _guard = layout_lock();
    let h = scene("wc-small", (1200.0, 700.0));
    let close = h.get_by_role_and_label(Role::Button, "Close").rect();
    let max = h.get_by_role_and_label(Role::Button, "Maximize").rect();
    assert!(close.right() <= 1200.0 && max.left() > 0.0);
    // nothing of the bar's right-hand side overlaps the legend on its left
    let legend_right = h
        .root()
        .children_recursive()
        .filter(|n| n.accesskit_node().role() == Role::Button && n.rect().center().y < 60.0 && n.rect().right() < max.left() - 100.0)
        .map(|n| n.rect().right())
        .fold(0.0_f32, f32::max);
    let reset_left = h
        .root()
        .children_recursive()
        .filter(|n| n.accesskit_node().role() == Role::Button && n.rect().center().y < 60.0 && n.rect().left() > legend_right)
        .map(|n| n.rect().left())
        .fold(f32::MAX, f32::min);
    assert!(reset_left >= legend_right, "no overlap");
}
