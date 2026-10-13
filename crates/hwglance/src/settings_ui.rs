//! The settings window: General, Sensors, Layout, Processes, History, Alerts, Bar widget and
//! Collector tabs.

use crate::a11y::{self, Spoken};
use crate::app::{source_label, App, ResetKind, SettingsTab};
use crate::collector_cfg;
use crate::conn;
use crate::fonts::FontChoice;
use crate::sensors::threshold_editor;
use crate::settings::{self, Settings, HISTORY_CHOICES};
use crate::theme::{base, fs, ThemeChoice};
use crate::thresholds::ThrEdit;
use crate::ui_kit::{f_icon, f_label_bold, icon, reset_button, ResetButton};
use egui::{vec2, Context, RichText, Ui};
use hwglance_proto::{self as proto, Reading, Snapshot, Unit};
use std::collections::HashMap;

/// Design width of the Settings window, and the smallest hit area of its close button.
pub const SETTINGS_W: f32 = 860.0;
pub const CLOSE_HIT: f32 = 24.0;

/// Values per card in the bar widget's popup.
pub const PER_CARD_MAX: usize = 5;
pub const PROC_ROWS: std::ops::RangeInclusive<u32> = 4..=15;

/// The three cards of the bar widget popup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PopupCard {
    Cpu,
    Gpu,
    Memory,
}

impl PopupCard {
    pub const ALL: [PopupCard; 3] = [PopupCard::Cpu, PopupCard::Gpu, PopupCard::Memory];

    pub fn title(self) -> &'static str {
        match self {
            PopupCard::Cpu => "CPU",
            PopupCard::Gpu => "GPU",
            PopupCard::Memory => "Memory",
        }
    }

    /// The card a reading id belongs to, by its prefix.
    pub fn of(id: &str) -> Option<PopupCard> {
        let head = id.split('/').next()?;
        match head {
            "cpu" => Some(PopupCard::Cpu),
            "memory" => Some(PopupCard::Memory),
            h if h.starts_with("gpu") => Some(PopupCard::Gpu),
            _ => None,
        }
    }
}

/// Split the flat popup list into per-card lists (order kept) and ids no card claims.
pub fn split_popup(list: &[String]) -> ([Vec<String>; 3], Vec<String>) {
    let mut cards: [Vec<String>; 3] = Default::default();
    let mut other = Vec::new();
    for id in list {
        match PopupCard::of(id) {
            Some(c) => cards[PopupCard::ALL.iter().position(|x| *x == c).unwrap()].push(id.clone()),
            None => other.push(id.clone()),
        }
    }
    (cards, other)
}

/// Join per-card lists back into the flat format the bar widget reads.
pub fn join_popup(cards: &[Vec<String>; 3], other: &[String]) -> Vec<String> {
    cards.iter().flatten().chain(other.iter()).cloned().collect()
}

/// All readings grouped by card, for pickers and name lookups. Section summaries that are not
/// copies of a reading (like `memory/temp/summary`) are included.
pub struct Catalog {
    /// (card id, card title, readings as (id, label, unit))
    pub cards: Vec<(String, String, Vec<(String, String, Unit)>)>,
    pub names: HashMap<String, String>,
    pub readings: HashMap<String, Reading>,
    /// Section id to "Card / Section", card id to its title: Settings names what is hidden.
    pub section_names: HashMap<String, String>,
    pub card_names: HashMap<String, String>,
}

pub fn catalog(snap: Option<&Snapshot>) -> Catalog {
    let mut c = Catalog {
        cards: Vec::new(),
        names: HashMap::new(),
        readings: HashMap::new(),
        section_names: HashMap::new(),
        card_names: HashMap::new(),
    };
    let Some(snap) = snap else { return c };
    for g in &snap.sensors {
        let mut list = Vec::new();
        c.card_names.insert(g.id.clone(), g.title.clone());
        // two readings of one card with the same label ("Core" under Load and under
        // Temperatures) are told apart by their section
        let mut labels: HashMap<&str, usize> = HashMap::new();
        for sec in &g.sections {
            for r in &sec.readings {
                *labels.entry(r.label.as_str()).or_default() += 1;
            }
        }
        for sec in &g.sections {
            c.section_names.insert(sec.id.clone(), format!("{} / {}", g.title, sec.title));
            for r in &sec.readings {
                list.push((r.id.clone(), format!("{}: {}", sec.title, r.label), r.unit));
                let name = if labels[r.label.as_str()] > 1 { format!("{} / {}: {}", g.title, sec.title, r.label) } else { format!("{} / {}", g.title, r.label) };
                c.names.insert(r.id.clone(), name);
                c.readings.insert(r.id.clone(), r.clone());
            }
            if let Some(sum) = &sec.summary {
                if !sec.readings.iter().any(|r| r.id == sum.id) {
                    list.push((sum.id.clone(), format!("{}: {} (summary)", sec.title, sum.label), sum.unit));
                    c.names.insert(sum.id.clone(), format!("{} / {} ({})", g.title, sum.label, sec.title));
                    c.readings.insert(sum.id.clone(), sum.clone());
                }
            }
        }
        c.cards.push((g.id.clone(), g.title.clone(), list));
    }
    // what the section did not part (same label twice in one section) takes the end of its id
    let mut count: HashMap<String, usize> = HashMap::new();
    for n in c.names.values() {
        *count.entry(n.clone()).or_default() += 1;
    }
    for (id, name) in c.names.iter_mut().filter(|(_, n)| count[*n] > 1) {
        name.push_str(&format!(" ({})", id.rsplit('/').next().unwrap_or(id)));
    }
    c
}

/// Move the item at `i` up or down by one.
pub fn move_item(list: &mut [String], i: usize, up: bool) {
    if up && i > 0 {
        list.swap(i, i - 1);
    } else if !up && i + 1 < list.len() {
        list.swap(i, i + 1);
    }
}

/// Approximate memory the collector needs for the history of every reading (about 2 MB at 30 minutes).
pub fn history_cost_mb(minutes: u32) -> f32 {
    minutes as f32 * 2.0 / 30.0
}

pub fn level_label(level: &str) -> &'static str {
    match level {
        "off" => "Off",
        "warning" => "Warning and critical",
        _ => "Critical only",
    }
}

pub fn scope_label(scope: &str) -> &'static str {
    match scope {
        "all" => "All values with thresholds",
        _ => "Temperatures only",
    }
}

impl App {
    pub(crate) fn settings_window(&mut self, ctx: &Context) {
        let was_open = std::mem::replace(&mut self.settings_was_open, self.settings_open);
        if !self.settings_open {
            return;
        }
        let width = fs(SETTINGS_W).min((ctx.content_rect().width() - 32.0).max(fs(560.0)));
        let mut window = egui::Window::new("Settings")
            .id(egui::Id::new("settings-window"))
            .title_bar(false)
            .default_size([width, fs(620.0)])
            .min_size([fs(560.0), fs(360.0)])
            .pivot(egui::Align2::RIGHT_TOP);
        // Each time it opens it sits under the gear, its right edge on the gear's; after that it
        // can be dragged where it suits.
        if let (false, Some(g)) = (was_open, self.gear_rect) {
            window = window.current_pos(egui::pos2(g.right(), g.bottom() + 6.0));
        }
        let mut close = false;
        let shown = window.show(ctx, |ui| {
            // The title bar is drawn here, so its close button can have a 24 px hit area.
            ui.horizontal(|ui| {
                ui.label(RichText::new("Settings").font(crate::ui_kit::f_title()).color(self.pal.ink));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let side = fs(CLOSE_HIT).max(CLOSE_HIT);
                    let b = egui::Button::new(RichText::new(icon::CLOSE.to_string()).font(f_icon(base() + 2.0)).color(self.pal.ink2))
                        .frame_when_inactive(false)
                        .min_size(vec2(side, side));
                    let r = ui.add(b).on_hover_text("Close (Esc)");
                    r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Close settings"));
                    if r.clicked() {
                        close = true;
                    }
                });
            });
            ui.horizontal(|ui| {
                for (tab, label) in [
                    (SettingsTab::General, "General"),
                    (SettingsTab::Sensors, "Sensors"),
                    (SettingsTab::Layout, "Layout"),
                    (SettingsTab::Processes, "Processes"),
                    (SettingsTab::History, "History"),
                    (SettingsTab::Alerts, "Alerts"),
                    (SettingsTab::BarWidget, "Bar widget"),
                    (SettingsTab::Collector, "Collector"),
                ] {
                    if ui.selectable_label(self.settings_tab == tab, label).clicked() {
                        self.settings_tab = tab;
                    }
                }
            });
            ui.separator();
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| match self.settings_tab {
                SettingsTab::General => self.tab_general(ui),
                SettingsTab::Sensors => self.tab_sensors(ui),
                SettingsTab::Layout => self.tab_layout(ui),
                SettingsTab::Processes => self.tab_processes(ui),
                SettingsTab::History => self.tab_history(ui),
                SettingsTab::Alerts => self.tab_alerts(ui),
                SettingsTab::BarWidget => self.tab_bar_widget(ui),
                SettingsTab::Collector => self.tab_collector(ui),
            });
        });
        if let Some(r) = shown {
            a11y::name_window_edges(ctx, r.response.layer_id, "Settings");
        }
        if close {
            self.settings_open = false;
        }
    }

    fn heading(&self, ui: &mut Ui, text: &str) {
        ui.label(RichText::new(text).font(crate::ui_kit::f_title()).color(self.pal.ink));
        ui.add_space(6.0);
    }

    fn tab_general(&mut self, ui: &mut Ui) {
        self.heading(ui, "General");
        ui.label(RichText::new("Theme").font(f_label_bold()));
        let mut theme = ThemeChoice::parse(&self.s.appearance.theme);
        ui.horizontal(|ui| {
            ui.radio_value(&mut theme, ThemeChoice::System, "Follow system");
            ui.radio_value(&mut theme, ThemeChoice::Dark, "Dark");
            ui.radio_value(&mut theme, ThemeChoice::Light, "Light");
        });
        self.s.appearance.theme = theme.as_str().to_string();
        ui.label(RichText::new("Follow system uses the desktop's light or dark setting, so switching the Omarchy theme switches this window.").color(self.pal.ink3));
        ui.add_space(8.0);
        ui.label(RichText::new("Font").font(f_label_bold()));
        let mut choice = FontChoice::parse(&self.s.appearance.font);
        ui.horizontal(|ui| {
            ui.radio_value(&mut choice, FontChoice::Atkinson, "Atkinson Hyperlegible");
            ui.radio_value(&mut choice, FontChoice::Omarchy, "Omarchy font");
        });
        self.s.appearance.font = choice.as_str().to_string();
        ui.label(RichText::new("Atkinson Hyperlegible is built in. The Omarchy font is whatever omarchy-font-current reports.").color(self.pal.ink3));
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label("Font size");
            crate::ui_kit::slider(ui, &self.pal, "Font size", egui::Slider::new(&mut self.s.appearance.font_size, 10.0..=24.0).step_by(0.5).suffix(" px"));
            if ui.button("Default").clicked() {
                self.s.appearance.font_size = settings::DEFAULT_FONT_SIZE;
            }
        });
        ui.label(RichText::new("Ctrl+= and Ctrl+- change it too, Ctrl+0 resets it. Everything else scales from this size.").color(self.pal.ink3));
        ui.add_space(14.0);
        ui.separator();
        ui.label(RichText::new("Reset all customisations").font(f_label_bold()));
        ui.label("Clears hidden and watched values, collapsed values, layout, process, theme and font settings. The bar widget and alert choices are kept, and so are the limits in thresholds.json.");
        let c = ResetButton {
            enabled: true,
            label: "Reset all customisations",
            tip: "Clear hidden and watched values, collapsed values, layout, process, theme and font settings.",
            disabled_tip: "",
            undo: self.undo_offer(ResetKind::Customisations),
        };
        if ui.horizontal(|ui| reset_button(ui, &self.pal, &c)).inner {
            self.reset_or_undo(ResetKind::Customisations);
        }
    }

    fn tab_sensors(&mut self, ui: &mut Ui) {
        let cat = catalog(self.snap.as_deref());
        let name = |id: &str| cat.names.get(id).cloned().unwrap_or_else(|| format!("{id} (not reported now)"));
        self.heading(ui, "Sensors");

        ui.label(RichText::new("Hidden").font(f_label_bold()));
        let mut show: Option<(u8, String)> = None;
        if self.s.sensors.hidden.is_empty() && self.s.sensors.hidden_sections.is_empty() && self.s.sensors.hidden_cards.is_empty() {
            ui.label(RichText::new("Nothing is hidden. Right-click a row, section or panel to hide it.").color(self.pal.ink3));
        }
        for id in &self.s.sensors.hidden {
            ui.horizontal(|ui| {
                if ui.button("Show").clicked() {
                    show = Some((0, id.clone()));
                }
                ui.label(name(id));
            });
        }
        for id in &self.s.sensors.hidden_sections {
            ui.horizontal(|ui| {
                if ui.button("Show").clicked() {
                    show = Some((1, id.clone()));
                }
                ui.label(format!("Section {}", cat.section_names.get(id).unwrap_or(id)));
            });
        }
        for id in &self.s.sensors.hidden_cards {
            ui.horizontal(|ui| {
                if ui.button("Show").clicked() {
                    show = Some((2, id.clone()));
                }
                ui.label(format!("Panel {}", cat.card_names.get(id).unwrap_or(id)));
            });
        }
        if let Some((kind, id)) = show {
            let list = match kind {
                0 => &mut self.s.sensors.hidden,
                1 => &mut self.s.sensors.hidden_sections,
                _ => &mut self.s.sensors.hidden_cards,
            };
            list.retain(|x| x != &id);
        }

        ui.add_space(12.0);
        ui.label(RichText::new("Watched").font(f_label_bold()));
        if self.s.sensors.highlighted.is_empty() {
            ui.label(RichText::new("Nothing is watched. Right-click a row and choose Watch.").color(self.pal.ink3));
        }
        let mut unwatch: Option<String> = None;
        for id in &self.s.sensors.highlighted {
            ui.horizontal(|ui| {
                if ui.button("Remove").clicked() {
                    unwatch = Some(id.clone());
                }
                ui.label(RichText::new(name(id)).color(self.pal.watch));
            });
        }
        if let Some(id) = unwatch {
            self.s.sensors.highlighted.retain(|x| x != &id);
        }

        ui.add_space(12.0);
        ui.label(RichText::new("Thresholds").font(f_label_bold()));
        ui.label(RichText::new("Changes made here are written to thresholds.json, which you can also edit by hand (see the Alerts tab).").color(self.pal.ink3));
        let mut edits: Vec<ThrEdit> = Vec::new();
        self.add_threshold(ui, &cat, &mut edits);
        let snap = self.snap.clone();
        let ids = snap.as_deref().map(|s| self.thr_pending.custom_ids(s)).unwrap_or_default();
        if ids.is_empty() {
            ui.label(RichText::new("No custom thresholds yet.").color(self.pal.ink3));
        }
        for id in ids {
            let title = name(&id);
            egui::CollapsingHeader::new(title).id_salt(("thr", &id)).show(ui, |ui| {
                if let (Some(r), Some(snap)) = (cat.readings.get(&id), snap.as_deref()) {
                    threshold_editor(ui, &self.pal, snap, &self.thr_pending, r, &mut edits);
                }
            });
        }
        self.send_threshold_edits(edits);

        ui.add_space(12.0);
        ui.label(RichText::new("Collapsed values").font(f_label_bold()));
        if self.s.sensors.summary_overrides.is_empty() {
            ui.label(RichText::new("All collapsed sections show their default value.").color(self.pal.ink3));
        }
        let mut reset: Option<String> = None;
        for (sec, rid) in &self.s.sensors.summary_overrides {
            ui.horizontal(|ui| {
                if ui.button("Reset").clicked() {
                    reset = Some(sec.clone());
                }
                ui.label(format!("{sec}: {}", name(rid)));
            });
        }
        if let Some(sec) = reset {
            self.s.sensors.summary_overrides.remove(&sec);
        }
    }

    /// "Add a threshold": search for any reading, pick it, then set its warning and critical
    /// values. The first edit stores the override, and it then shows in the list below.
    fn add_threshold(&mut self, ui: &mut Ui, cat: &Catalog, edits: &mut Vec<ThrEdit>) {
        match self.thr_pick.clone() {
            None => {
                let search = egui::TextEdit::singleline(&mut self.thr_search)
                    .id(egui::Id::new("thr-search"))
                    .hint_text("Add a threshold: search for a value, such as Tctl or fan")
                    .desired_width(fs(380.0))
                    .margin(vec2(8.0, 4.0))
                    .min_size(vec2(0.0, crate::theme::MIN_HIT));
                let found = ui.add(search);
                a11y::publish(&found, || Spoken::new(egui::accesskit::Role::SearchInput, "Search for a value to add a threshold"));
                let query = self.thr_search.trim().to_lowercase();
                if query.is_empty() {
                    return;
                }
                let matches = threshold_matches(cat, &query, THR_MATCHES);
                if matches.is_empty() {
                    ui.label(RichText::new("No value matches.").color(self.pal.ink3));
                }
                for (id, label) in matches {
                    if ui.selectable_label(false, label).clicked() {
                        self.thr_pick = Some(id);
                        self.thr_search.clear();
                    }
                }
            }
            Some(id) => {
                let Some(r) = cat.readings.get(&id) else {
                    self.thr_pick = None;
                    return;
                };
                ui.horizontal(|ui| {
                    ui.label(RichText::new(cat.names.get(&id).cloned().unwrap_or(id.clone())).strong());
                    if ui.button("Done").clicked() {
                        self.thr_pick = None;
                    }
                });
                if let Some(snap) = self.snap.as_deref() {
                    threshold_editor(ui, &self.pal, snap, &self.thr_pending, r, edits);
                }
            }
        }
        ui.add_space(4.0);
    }

    fn tab_layout(&mut self, ui: &mut Ui) {
        self.heading(ui, "Layout");
        let l = &self.s.layout;
        if l.auto {
            ui.label("The panels are arranged automatically: as many columns as fit, balanced by row count.");
        } else {
            ui.label(format!("Custom layout: {} columns. Use Edit layout in the top bar to change it. One arrangement is shared by every dock position and view of the process panel. Columns shrink to fit, and neighbouring columns stack when the space is too narrow.", l.columns.len()));
        }
        ui.label(
            RichText::new(format!("{} sections and {} readings have a custom order.", l.section_order.len(), l.reading_order.len()))
                .color(self.pal.ink3),
        );
        ui.add_space(8.0);
        let can_reset = !l.auto || !l.section_order.is_empty() || !l.reading_order.is_empty();
        let c = ResetButton {
            enabled: can_reset,
            label: "Reset to automatic",
            tip: "Back to the automatic arrangement: clears your columns and the order of sections and readings.",
            disabled_tip: "",
            undo: self.undo_offer(ResetKind::Layout),
        };
        if ui.horizontal_wrapped(|ui| reset_button(ui, &self.pal, &c)).inner {
            self.reset_or_undo(ResetKind::Layout);
        }
        ui.label(RichText::new("Collapsed panels and sections are kept.").color(self.pal.ink3));
    }

    fn tab_processes(&mut self, ui: &mut Ui) {
        self.heading(ui, "Processes");
        ui.horizontal(|ui| {
            ui.label("Panel position");
            let mut dock = self.s.layout.dock();
            ui.selectable_value(&mut dock, crate::settings::Dock::Bottom, "Below the sensors");
            ui.selectable_value(&mut dock, crate::settings::Dock::Right, "At the right");
            if dock != self.s.layout.dock() {
                self.s.layout.set_dock(dock);
            }
        });
        ui.label(RichText::new("The sensor panels keep one arrangement for every position and view.").color(self.pal.ink3));
        ui.add_space(6.0);
        // The four views: compact and expanded for each dock. The checklist edits the current one.
        ui.label(RichText::new("Views").font(f_label_bold()));
        ui.label(
            RichText::new("Each dock position has a compact and an expanded view, switched with the arrow button in the panel header, by double-clicking the divider or with Ctrl+E. Every view keeps its own size, columns and Name width.")
                .color(self.pal.ink3),
        );
        let current = (self.s.layout.dock(), self.s.layout.view());
        egui::Grid::new("proc-views").num_columns(3).spacing([16.0, 4.0]).show(ui, |ui| {
            for (dock, view) in settings::ProcViews::ALL {
                let v = self.s.processes.views.get(dock, view).clone();
                let name = format!("{} {}", if dock == settings::Dock::Bottom { "Below" } else { "Right" }, view.as_str());
                if ui.selectable_label(current == (dock, view), name).on_hover_text("Switch to this view").clicked() {
                    self.s.layout.set_dock(dock);
                    self.s.layout.set_view(view);
                }
                let size = match (v.width_px, v.size) {
                    (Some(px), _) => format!("{px:.0} px wide"),
                    (None, Some(f)) => format!("{:.0} % of the {}", f * 100.0, if dock == settings::Dock::Bottom { "height" } else { "width" }),
                    _ => "automatic".to_string(),
                };
                ui.label(RichText::new(size).color(self.pal.ink2));
                let cols: Vec<&str> = crate::model::visible_columns(&v.columns).iter().map(|k| k.title()).collect();
                ui.label(RichText::new(format!("Name, {}", cols.join(", "))).color(self.pal.ink3));
                ui.end_row();
            }
        });
        let c = ResetButton {
            enabled: true,
            label: "Reset views",
            tip: "Back to the default sizes, columns and Name widths of all four views.",
            disabled_tip: "",
            undo: self.undo_offer(ResetKind::ProcViews),
        };
        if ui.horizontal_wrapped(|ui| reset_button(ui, &self.pal, &c)).inner {
            self.reset_or_undo(ResetKind::ProcViews);
        }
        ui.add_space(8.0);
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.label(RichText::new("Columns of the current view").font(f_label_bold()));
                let cols = self.s.current_view().columns.clone();
                if let Some(c) = crate::procs::column_checklist(ui, &cols) {
                    self.s.current_view_mut().columns = c;
                }
                if ui.button("Default columns").clicked() {
                    let d = settings::ProcViews::default();
                    let (dock, view) = (self.s.layout.dock(), self.s.layout.view());
                    self.s.current_view_mut().columns = d.get(dock, view).columns.clone();
                }
            });
            ui.add_space(24.0);
            ui.vertical(|ui| {
                ui.label(RichText::new("Right-click the table header to change the columns of the current view without opening Settings. Columns keep their fixed order, and Location, when shown, takes the width that is left.").color(self.pal.ink3));
            });
        });
        ui.add_space(8.0);
        let pal = self.pal.clone();
        let p = &mut self.s.processes;
        ui.checkbox(&mut p.hold_order_on_hover, "Hold the row order while the pointer is over the list")
            .on_hover_text("Values keep updating, but rows do not jump, so you can read a row you are looking at.");
        ui.horizontal(|ui| {
            ui.label("Re-sort at most every");
            crate::ui_kit::slider(ui, &pal, "Re-sort interval", egui::Slider::new(&mut p.resort_seconds, 0.0..=30.0).step_by(1.0).suffix(" s"));
        });
        ui.label(RichText::new("0 sorts on every update. This also applies when the pointer is elsewhere.").color(self.pal.ink3));
        ui.horizontal(|ui| {
            ui.label("Smooth CPU and GPU over");
            crate::ui_kit::slider(ui, &pal, "Smoothing", egui::Slider::new(&mut p.smoothing_seconds, 0.0..=15.0).step_by(0.5).suffix(" s"));
        });
        ui.label(RichText::new("Exponential moving average, used for display and sorting. 0 shows raw values.").color(self.pal.ink3));
        let changed = ui.ctx().input(|i| i.pointer.any_released());
        if changed {
            self.force_resort = true;
            self.rows_dirty = true;
        }
    }

    fn tab_history(&mut self, ui: &mut Ui) {
        self.heading(ui, "History");
        ui.horizontal(|ui| {
            ui.label("Keep the last");
            egui::ComboBox::from_id_salt("history-minutes")
                .selected_text(minutes_label(self.s.sensors.history_minutes))
                .show_ui(ui, |ui| {
                    for m in HISTORY_CHOICES {
                        ui.selectable_value(&mut self.s.sensors.history_minutes, m, minutes_label(m));
                    }
                });
        });
        ui.add_space(6.0);
        let m = self.s.sensors.history_minutes;
        ui.label(
            RichText::new(format!(
                "Every value is recorded, and hovering a row plots it. The collector needs about {:.1} MB for the last {}.",
                history_cost_mb(m),
                minutes_label(m)
            ))
            .color(self.pal.ink2),
        );
        ui.label(RichText::new("The collector reads this setting from ui.json.").color(self.pal.ink3));
    }

    fn tab_alerts(&mut self, ui: &mut Ui) {
        self.heading(ui, "Alerts");
        ui.label(RichText::new("The collector sends a desktop notification when a value crosses a threshold. Clicking it brings that value into view here.").color(self.pal.ink2));
        ui.add_space(8.0);
        // label and control in two columns, so the three controls share one left edge
        egui::Grid::new("alerts-grid").num_columns(2).spacing(vec2(12.0, 8.0)).show(ui, |ui| {
            grid_label(ui, "Notify for");
            egui::ComboBox::from_id_salt("alert-level")
                .selected_text(level_label(&self.s.alerts.level))
                .show_ui(ui, |ui| {
                    for l in ["off", "critical", "warning"] {
                        ui.selectable_value(&mut self.s.alerts.level, l.to_string(), level_label(l));
                    }
                });
            ui.end_row();
            grid_label(ui, "Which values");
            egui::ComboBox::from_id_salt("alert-scope")
                .selected_text(scope_label(&self.s.alerts.scope))
                .show_ui(ui, |ui| {
                    for sc in ["temperatures", "all"] {
                        ui.selectable_value(&mut self.s.alerts.scope, sc.to_string(), scope_label(sc));
                    }
                });
            ui.end_row();
            grid_label(ui, "Do not repeat within");
            ui.add(egui::DragValue::new(&mut self.s.alerts.cooldown_minutes).range(1..=240).suffix(" min"));
            ui.end_row();
        });
        ui.add_space(10.0);
        if ui.button(format!("{}  Send a test notification", icon::BELL)).clicked() {
            send_test_notification();
            self.set_toast("Test notification sent".into(), true);
        }
        self.limits_file_section(ui);
    }

    /// Where the warning and critical limits live, how to edit them, and what is wrong with the file
    /// when the collector could not read it.
    fn limits_file_section(&mut self, ui: &mut Ui) {
        ui.add_space(14.0);
        ui.separator();
        ui.label(RichText::new("Limits file").font(f_label_bold()));
        ui.label(RichText::new("The warning and critical limits of every value live in this file. Edit it in any text editor: changes apply within a few seconds.").color(self.pal.ink2));
        let path = settings::thresholds_path();
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(path.display().to_string()).monospace().color(self.pal.ink3));
            if ui.button("Open file").on_hover_text("Open thresholds.json with your default editor").clicked()
                && let Err(e) = crate::actions::open_path(&path.display().to_string())
            {
                self.set_toast(format!("Could not open the file: {e}"), false);
            }
        });
        if let Some(err) = self.snap.as_ref().and_then(|s| s.limits_error.clone()) {
            ui.add_space(6.0);
            ui.label(RichText::new("The file has an error").font(f_label_bold()).color(self.pal.crit));
            ui.label(RichText::new(&err).color(self.pal.crit));
            ui.label(RichText::new("It was not applied and is left as it is: the last good limits stay in force until you fix it.").color(self.pal.ink2));
        }
    }

    fn tab_bar_widget(&mut self, ui: &mut Ui) {
        let cat = catalog(self.snap.as_deref());
        let name = |id: &str| cat.names.get(id).cloned().unwrap_or_else(|| id.to_string());
        self.heading(ui, "Bar widget");
        ui.label(
            RichText::new("The bar button shows an icon or live usage, as chosen below. Its popup shows three cards: choose what each one lists. The first value of a card is its big one.")
                .color(self.pal.ink2),
        );
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            ui.label("Bar button");
            let mut mode = self.s.tray.bar_mode().to_string();
            for (v, l) in [("icon", "Icon"), ("cpu", "CPU usage"), ("gpu", "GPU usage"), ("cpu_gpu", "CPU and GPU usage")] {
                ui.radio_value(&mut mode, v.to_string(), l);
            }
            if mode != self.s.tray.bar_mode() {
                self.s.tray.bar_display = mode;
            }
        });
        ui.label(RichText::new("The percentages update about every 2 seconds. The icon returns when the collector is not running.").color(self.pal.ink3));
        ui.add_space(8.0);
        let (mut cards, other) = split_popup(&self.s.tray.popup_values);
        let mut changed = false;
        // a little slack: three columns that round up by a hair would widen the whole window
        ui.scope(|ui| {
            ui.set_max_width(ui.available_width() - 4.0);
            ui.columns(3, |cols| {
                for (i, card) in PopupCard::ALL.iter().enumerate() {
                    let ui = &mut cols[i];
                    let list = &mut cards[i];
                    ui.label(RichText::new(format!("{} ({})", card.title(), count_of(list.len(), PER_CARD_MAX))).font(f_label_bold()));
                    changed |= ordered_list(ui, list, &name, card.title());
                    if list.len() < PER_CARD_MAX {
                        let key = card.title();
                        egui::ComboBox::from_id_salt(("popup-add", key))
                            .selected_text("Add a value")
                            .width(ui.available_width() - 8.0)
                            .show_ui(ui, |ui| {
                                for (_, _, readings) in cat.cards.iter().filter(|(cid, _, _)| PopupCard::of(cid) == Some(*card)) {
                                    for (id, label, _) in readings {
                                        if list.contains(id) {
                                            continue;
                                        }
                                        if ui.selectable_label(false, label).clicked() {
                                            list.push(id.clone());
                                            changed = true;
                                        }
                                    }
                                }
                            });
                    }
                }
            });
        });
        if changed {
            self.s.tray.popup_values = join_popup(&cards, &other);
        }
        ui.add_space(12.0);
        egui::Grid::new("popup-grid").num_columns(2).spacing(vec2(12.0, 8.0)).show(ui, |ui| {
            grid_label(ui, "Process rows in the popup");
            crate::ui_kit::slider(ui, &self.pal, "Processes shown in the bar popup", egui::Slider::new(&mut self.s.tray.popup_process_count, PROC_ROWS));
            ui.end_row();
            grid_label(ui, "Popup sorted by");
            egui::ComboBox::from_id_salt("popup-sort")
                .selected_text(sort_label(&self.s.tray.popup_sort))
                .show_ui(ui, |ui| {
                    for (v, l) in [("cpu", "CPU"), ("gpu", "GPU"), ("memory", "Memory")] {
                        ui.selectable_value(&mut self.s.tray.popup_sort, v.to_string(), l);
                    }
                });
            ui.end_row();
        });
    }

    fn tab_collector(&mut self, ui: &mut Ui) {
        self.heading(ui, "Collector");
        let cfg = collector_cfg::config_path();
        let current = collector_cfg::read_interval(&cfg).or(self.snap.as_ref().map(|s| s.interval_ms));
        let mut edit = self.interval_edit.unwrap_or_else(|| current.unwrap_or(1000));
        ui.horizontal(|ui| {
            ui.label("Sample interval");
            let r = ui.add(
                egui::DragValue::new(&mut edit)
                    .range(collector_cfg::MIN_INTERVAL_MS..=collector_cfg::MAX_INTERVAL_MS)
                    .speed(10.0)
                    .suffix(" ms"),
            );
            if r.changed() {
                self.interval_edit = Some(edit);
            }
            let dirty = Some(edit) != current;
            if ui
                .add_enabled(dirty, egui::Button::new("Apply and restart collector"))
                .on_hover_text("Writes interval_ms to config.toml (other keys and comments stay), then runs systemctl --user restart hwglanced")
                .clicked()
            {
                match collector_cfg::write_interval(&cfg, edit) {
                    Ok(()) => {
                        conn::restart_collector(&self.link, ui.ctx());
                        self.interval_edit = None;
                        self.set_toast(format!("Interval set to {edit} ms, restarting the collector"), true);
                    }
                    Err(e) => self.set_toast(format!("Could not write config.toml: {}", settings::save_error_text(&e, &cfg)), false),
                }
            }
        });
        ui.label(RichText::new("Between 100 and 60000 ms. The window reconnects by itself after the restart.").color(self.pal.ink3));

        ui.add_space(12.0);
        ui.label(RichText::new("Sources").font(f_label_bold()));
        if let Some(snap) = self.snap.clone() {
            for s in &snap.sources {
                ui.horizontal(|ui| {
                    let color = if s.ok { self.pal.ink3 } else { self.pal.warn };
                    ui.label(RichText::new(if s.ok { "ok" } else { "not ok" }).color(color).monospace());
                    ui.label(RichText::new(source_label(&s.name)).strong());
                    ui.label(RichText::new(&s.detail).color(self.pal.ink3));
                });
            }
        } else {
            ui.label(RichText::new("No snapshot yet.").color(self.pal.ink3));
        }

        ui.add_space(12.0);
        ui.label(RichText::new("Files").font(f_label_bold()));
        let paths = [
            ("Window settings", self.settings_path.clone()),
            ("Collector config", cfg),
            ("Limits file", settings::thresholds_path()),
            ("Collector socket", proto::socket_path()),
            ("Summary file", proto::summary_path()),
        ];
        egui::Grid::new("paths").num_columns(2).spacing([16.0, 4.0]).show(ui, |ui| {
            for (label, p) in paths {
                ui.label(label);
                ui.label(RichText::new(p.display().to_string()).monospace().color(self.pal.ink3));
                ui.end_row();
            }
        });
    }
}

/// The label of a settings row laid in a grid: centred on the height of the control beside it.
fn grid_label(ui: &mut Ui, text: &str) {
    ui.horizontal(|ui| {
        ui.set_min_height(ui.spacing().interact_size.y);
        ui.label(text);
    });
}

/// A counter against its limit, the way every counter in the window is written: "3 of 5".
pub fn count_of(n: usize, of: usize) -> String {
    format!("{n} of {of}")
}

/// How many matches the threshold search lists.
const THR_MATCHES: usize = 8;

/// Readings whose name contains `query` (lower case), with a threshold to set: flags have none.
/// Returns (id, "Card / Reading") pairs, at most `max`.
pub fn threshold_matches(cat: &Catalog, query: &str, max: usize) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (_, _, readings) in &cat.cards {
        for (id, label, unit) in readings {
            if *unit == Unit::Flag {
                continue;
            }
            let name = cat.names.get(id).cloned().unwrap_or_else(|| label.clone());
            if name.to_lowercase().contains(query) || label.to_lowercase().contains(query) {
                out.push((id.clone(), name));
                if out.len() >= max {
                    return out;
                }
            }
        }
    }
    out
}

fn minutes_label(m: u32) -> String {
    if m >= 60 { format!("{} h", m / 60) } else { format!("{m} minutes") }
}

fn sort_label(v: &str) -> &'static str {
    match v {
        "gpu" => "GPU",
        "memory" => "Memory",
        _ => "CPU",
    }
}

/// `notify-send` with the test text, off the UI thread.
pub fn send_test_notification() {
    std::thread::spawn(|| {
        let _ = std::process::Command::new("notify-send")
            .args(["-a", "HW Glance", "-i", "utilities-system-monitor", "HW Glance test", "Alerts will look like this."])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    });
}

/// An ordered list with move up, move down and remove buttons. Returns true when it changed.
fn ordered_list(ui: &mut Ui, list: &mut Vec<String>, name: &dyn Fn(&str) -> String, salt: &str) -> bool {
    if list.is_empty() {
        ui.label(RichText::new("Nothing chosen").weak());
    }
    let mut action: Option<(usize, u8)> = None;
    for (i, id) in list.iter().enumerate() {
        ui.push_id((salt, i), |ui| {
            ui.horizontal(|ui| {
                // 24 px squares: the glyphs stay small, the hit area does not
                let square = |g: char| {
                    egui::Button::new(RichText::new(g.to_string()).font(crate::ui_kit::f_icon(crate::theme::base())))
                        .min_size(vec2(crate::theme::MIN_HIT, crate::theme::MIN_HIT))
                };
                let full = name(id);
                let up = ui.add_enabled(i > 0, square(icon::ARROW_UP));
                a11y::publish(&up, || Spoken::button(format!("Move {full} up")));
                if up.clicked() {
                    action = Some((i, 0));
                }
                let down = ui.add_enabled(i + 1 < list.len(), square(icon::ARROW_DOWN));
                a11y::publish(&down, || Spoken::button(format!("Move {full} down")));
                if down.clicked() {
                    action = Some((i, 1));
                }
                let remove = ui.add(square(icon::CLOSE));
                a11y::publish(&remove, || Spoken::button(format!("Remove {full} from the list")));
                if remove.clicked() {
                    action = Some((i, 2));
                }
                // a long name ends in an ellipsis, so the column never widens the Settings window
                ui.add(egui::Label::new(full.as_str()).truncate()).on_hover_text(full);
            });
        });
    }
    match action {
        Some((i, 0)) => move_item(list, i, true),
        Some((i, 1)) => move_item(list, i, false),
        Some((i, _)) => {
            list.remove(i);
        }
        None => return false,
    }
    true
}

#[allow(dead_code)]
fn _assert_settings_send(_: &Settings) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(xs: &[&str]) -> Vec<String> {
        xs.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_default_popup_lists_fit_their_per_card_limit() {
        // the counters read "n of m": a default list may never print "5 of 4"
        let (cards, other) = split_popup(&crate::settings::TrayPrefs::default().popup_values);
        assert!(other.is_empty());
        for (card, list) in PopupCard::ALL.iter().zip(&cards) {
            assert!(list.len() <= PER_CARD_MAX, "{}: {} values", card.title(), list.len());
        }
    }

    #[test]
    fn popup_cards_by_prefix() {
        assert_eq!(PopupCard::of("cpu/temp/tctl"), Some(PopupCard::Cpu));
        assert_eq!(PopupCard::of("gpu0/load/core"), Some(PopupCard::Gpu));
        assert_eq!(PopupCard::of("gpu1/temp/core"), Some(PopupCard::Gpu));
        assert_eq!(PopupCard::of("memory/temp/summary"), Some(PopupCard::Memory));
        assert_eq!(PopupCard::of("board/temp/vrm"), None);
    }

    #[test]
    fn the_flat_list_splits_per_card_and_joins_back() {
        let flat = ids(&[
            "cpu/usage/total",
            "gpu0/load/core",
            "cpu/temp/tctl",
            "memory/usage/pct",
            "board/temp/vrm",
            "gpu0/temp/core",
        ]);
        let (cards, other) = split_popup(&flat);
        assert_eq!(cards[0], ids(&["cpu/usage/total", "cpu/temp/tctl"]));
        assert_eq!(cards[1], ids(&["gpu0/load/core", "gpu0/temp/core"]));
        assert_eq!(cards[2], ids(&["memory/usage/pct"]));
        assert_eq!(other, ids(&["board/temp/vrm"]));
        // joined per card, in card order, with unclaimed ids kept at the end
        assert_eq!(
            join_popup(&cards, &other),
            ids(&["cpu/usage/total", "cpu/temp/tctl", "gpu0/load/core", "gpu0/temp/core", "memory/usage/pct", "board/temp/vrm"])
        );
        // the defaults are already in card order and survive a round trip
        let d = settings::TrayPrefs::default().popup_values;
        let (c, o) = split_popup(&d);
        assert_eq!(join_popup(&c, &o), d);
        assert!(o.is_empty());
        assert_eq!((c[0].len(), c[1].len(), c[2].len()), (4, 5, 3), "the default list from the brief");
    }

    #[test]
    fn summary_ids_are_in_the_catalog() {
        let d = crate::demo::Demo::new().snapshot(5.0);
        let cat = catalog(Some(&d));
        // a derived summary (not a copy of a reading) is pickable
        assert!(cat.readings.contains_key("memory/temp/summary"), "{:?}", cat.names.keys().filter(|k| k.contains("summary")).collect::<Vec<_>>());
        let mem = cat.cards.iter().find(|(id, _, _)| id == "memory").unwrap();
        assert!(mem.2.iter().any(|(id, label, _)| id == "memory/temp/summary" && label.contains("summary")));
        // and a plain reading is too
        assert!(cat.readings.contains_key("cpu/temp/tctl"));
    }

    #[test]
    fn move_items() {
        let mut l = ids(&["a", "b", "c"]);
        move_item(&mut l, 2, true);
        assert_eq!(l, ids(&["a", "c", "b"]));
        move_item(&mut l, 0, true);
        move_item(&mut l, 2, false);
        assert_eq!(l, ids(&["a", "c", "b"]));
        move_item(&mut l, 0, false);
        assert_eq!(l, ids(&["c", "a", "b"]));
    }

    #[test]
    fn history_cost_and_labels() {
        assert!((history_cost_mb(30) - 2.0).abs() < 1e-6);
        assert!((history_cost_mb(240) - 16.0).abs() < 1e-6);
        assert_eq!(minutes_label(5), "5 minutes");
        assert_eq!(minutes_label(120), "2 h");
        assert_eq!(level_label("off"), "Off");
        assert_eq!(level_label("critical"), "Critical only");
        assert_eq!(level_label("warning"), "Warning and critical");
        assert_eq!(scope_label("all"), "All values with thresholds");
        assert_eq!(scope_label("temperatures"), "Temperatures only");
    }

    fn reading(id: &str, label: &str) -> Reading {
        Reading { id: id.into(), label: label.into(), unit: Unit::Percent, value: Some(1.0), stats: None, note: None, desc: None, events: None, warn: None, crit: None }
    }

    fn section(id: &str, title: &str, readings: Vec<Reading>) -> hwglance_proto::Section {
        hwglance_proto::Section { id: id.into(), title: title.into(), collapsed_by_default: false, readings, summary: None }
    }

    #[test]
    fn two_readings_with_one_label_are_told_apart_by_their_section() {
        let mut snap = crate::demo::Demo::new().snapshot(1.0);
        snap.sensors = vec![hwglance_proto::SensorGroup {
            id: "gpu0".into(),
            title: "GPU".into(),
            subtitle: None,
            sections: vec![
                section("gpu0/load", "Load", vec![reading("gpu0/load/core", "Core"), reading("gpu0/load/memctl", "Memory controller")]),
                section("gpu0/temp", "Temperatures", vec![reading("gpu0/temp/core", "Core")]),
            ],
        }];
        let cat = catalog(Some(&snap));
        assert_eq!(cat.names["gpu0/load/core"], "GPU / Load: Core");
        assert_eq!(cat.names["gpu0/temp/core"], "GPU / Temperatures: Core");
        assert_eq!(cat.names["gpu0/load/memctl"], "GPU / Memory controller", "a label that is alone stays short");
        let all: std::collections::HashSet<_> = cat.names.values().collect();
        assert_eq!(all.len(), cat.names.len(), "no two readings share a name");
        assert_eq!(cat.section_names["gpu0/temp"], "GPU / Temperatures");
        assert_eq!(cat.card_names["gpu0"], "GPU");
    }

    #[test]
    fn the_demo_snapshot_has_no_two_readings_with_one_name() {
        let cat = catalog(Some(&crate::demo::Demo::new().snapshot(1.0)));
        let mut seen = std::collections::HashMap::new();
        for (id, name) in &cat.names {
            if let Some(other) = seen.insert(name.clone(), id.clone()) {
                panic!("{name} is both {id} and {other}");
            }
        }
    }

    #[test]
    fn counters_read_n_of_m() {
        assert_eq!(count_of(3, 5), "3 of 5");
        assert_eq!(count_of(7, 5), "7 of 5");
    }
}
