//! Application state, top bar, status bar, shortcuts, the two-step resets, the kill confirmation
//! and the check that follows a kill.

use crate::a11y::{self, Ring, Spoken};
use crate::actions::{self, Signal};
use crate::conn::{self, Link};
use crate::crossings;
use crate::icons::IconCache;
use crate::layout;
use crate::model::{self, OrderMemory, Row, RowId, Smoother, SortKey, ViewParams};
use crate::settings::{self, Dock, Settings};
use crate::fonts::{self, FontChoice};
use crate::history::{self, PopInput};
use crate::system_theme::{self, SystemLight};
use crate::theme::{self, base, fs, Palette, ThemeChoice};
use crate::winctl;
use crate::ui_kit::{self, confirm_button, f_label, f_label_bold, f_small, f_title, icon, Confirm, ConfirmStep, Device};
use crate::sensors::{self, DeviceState, Hit, ViewOut};
use crate::thresholds::{Pending, ThrEdit};
use eframe::egui;
use egui::{pos2, vec2, Align, Color32, Event, FontId, Key, Layout, Margin, Modifiers, RichText, Sense, Stroke, Ui};
use hwglance_proto::{format_value, ClientMsg, ProcGroup, Snapshot, Unit};
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

/// Key of the first version's eframe-storage state, migrated into ui.json once.
pub const LEGACY_STORAGE_KEY: &str = "hwglance-state-v1";
pub const SEARCH_ID: &str = "proc-search";
const SAVE_DEBOUNCE: Duration = Duration::from_millis(500);
/// How long after a failed save the next try waits (a full disk or a read-only folder rarely
/// clears in a second, and the banner has a button for an immediate retry).
const SAVE_RETRY: Duration = Duration::from_secs(10);
const HOVER_DELAY: Duration = Duration::from_millis(350);
/// How long a reset stays armed, waiting for its second click.
const ARMED_FOR: Duration = Duration::from_secs(8);
/// How long after a kill the target's processes are checked.
const KILL_CHECK_FOR: Duration = Duration::from_secs(3);
/// How long the outcome stays in the action row (it also goes when the selection changes).
const KILL_OUTCOME_FOR: Duration = Duration::from_secs(10);

#[derive(Clone, Debug)]
pub enum Mode {
    Live,
    Demo,
    Mock(PathBuf),
}

/// Start-up options besides the data source.
#[derive(Clone, Debug, Default)]
pub struct Opts {
    /// Override of the ui.json location (tests).
    pub settings_path: Option<PathBuf>,
    pub open_settings: bool,
    /// Do not start the file watchers (tests).
    pub no_watchers: bool,
    /// Reading to bring into view and pin when the first snapshot arrives (`--focus`).
    pub focus: Option<String>,
}

/// The first version's persisted state. Plain fields (no `Option`), because eframe stores it
/// as RON, which writes `Some(..)` for options and would not read the bare values it wrote.
#[derive(Deserialize)]
#[serde(default)]
struct LegacyState {
    split: f32,
    sort: SortKey,
    sort_desc: bool,
    section_open: HashMap<String, bool>,
    proc_sections: HashMap<String, bool>,
    procs_visible: bool,
}

impl Default for LegacyState {
    fn default() -> Self {
        LegacyState {
            split: 0.45,
            sort: SortKey::Cpu,
            sort_desc: true,
            section_open: HashMap::new(),
            proc_sections: HashMap::new(),
            procs_visible: true,
        }
    }
}

pub struct PendingKill {
    /// The row the action was asked for (the outcome is shown while it is selected).
    pub row: RowId,
    pub signal: Signal,
    pub target: model::Target,
    pub warning: Option<String>,
    /// Keyboard focus was put on Cancel when the dialog opened.
    pub focus_set: bool,
}

/// The resets that ask twice. Only one is armed at a time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResetKind {
    /// The top bar's Reset and Ctrl+R: low, high, average, throttle history and finished crossings.
    Stats,
    /// The edit bar's Reset layout.
    EditLayout,
    /// Settings, Layout: Reset to automatic.
    LayoutAuto,
    /// Settings, Processes: Reset views.
    ProcViews,
}

/// What follows ending a task: the target's processes are checked for a few seconds and the
/// outcome stays in the process panel's action row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KillState {
    /// Other users' processes: polkit has not answered yet.
    Waiting,
    /// Signalled; the processes are being checked.
    Checking,
    Ended,
    StillRunning,
    /// Authentication was dismissed or the signal failed (the text is in `message`).
    Failed,
}

pub struct KillWatch {
    /// The row that was selected when the signal was sent.
    pub row: RowId,
    pub name: String,
    pub signal: Signal,
    pub procs: Vec<model::TargetProc>,
    pub state: KillState,
    /// When the check began, then when the outcome was reached.
    pub since: Instant,
    pub message: Option<String>,
}

/// Is this process still running? Overridden in the tests, which must never look at real ones.
pub type AliveCheck = fn(&model::TargetProc) -> bool;

/// Why the sensor area has no snapshot to show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Waiting {
    /// The collector does not answer (live mode only): say so and offer to start it.
    NotRunning,
    /// Connected, the first snapshot has not arrived yet.
    FirstSnapshot,
    /// Still trying to connect (nothing has failed yet, or not a live source).
    Connecting,
}

pub fn waiting_state(live: bool, connected: bool, connect_failed: bool) -> Waiting {
    if live && connect_failed && !connected {
        Waiting::NotRunning
    } else if connected {
        Waiting::FirstSnapshot
    } else {
        Waiting::Connecting
    }
}

/// Health of the collector's data sources, for the Sources chip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceHealth {
    Healthy,
    Degraded,
    Down,
}

pub fn source_health(ok: usize, total: usize) -> SourceHealth {
    if total == 0 || ok >= total {
        SourceHealth::Healthy
    } else if ok == 0 {
        SourceHealth::Down
    } else {
        SourceHealth::Degraded
    }
}

/// Dot colour of the chip. Green belongs to watched values, so a healthy chip stays quiet.
pub fn source_dot_color(h: SourceHealth, pal: &Palette) -> Color32 {
    match h {
        SourceHealth::Healthy => pal.ink3,
        SourceHealth::Degraded => pal.warn,
        SourceHealth::Down => pal.crit,
    }
}

/// The notice that ui.json could not be read: shown until dismissed, then never again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CorruptNotice {
    /// Where the unreadable file is (or will be) kept.
    pub bad: PathBuf,
    /// The copy has been made (the first save after the load makes it).
    pub saved: bool,
}

impl CorruptNotice {
    /// The one sentence the banner shows.
    pub fn text(&self, home: &str) -> String {
        let bad = model::display_path(&self.bad.display().to_string(), home);
        if self.saved {
            format!("Your settings file could not be read, so HW Glance went back to its defaults. The unreadable file is saved as {bad}.")
        } else {
            format!("Your settings file could not be read, so HW Glance started with its defaults. The unreadable file is kept as {bad} when the first change is saved.")
        }
    }
}

/// The copy `save_atomic` makes of an unreadable settings file.
pub fn bad_copy_path(settings: &std::path::Path) -> PathBuf {
    let mut bak = settings.as_os_str().to_owned();
    bak.push(".bad");
    PathBuf::from(bak)
}

pub struct Toast {
    pub text: String,
    pub ok: bool,
    pub until: Instant,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SettingsTab {
    General,
    Sensors,
    Layout,
    Processes,
    History,
    Alerts,
    BarWidget,
    Collector,
}

pub struct App {
    // Settings (ui.json)
    pub s: Settings,
    s_saved: Settings,
    s_seen: Settings,
    pub settings_path: PathBuf,
    last_written: Option<SystemTime>,
    dirty_since: Option<Instant>,
    corrupt: bool,
    /// The banner about an unreadable ui.json; `None` once dismissed or when it was readable.
    pub corrupt_notice: Option<CorruptNotice>,
    pub external_change: Arc<AtomicBool>,
    /// Why the last save of ui.json failed, while it keeps failing (a banner says so).
    pub save_error: Option<String>,
    applied_font_size: f32,
    applied_font: FontChoice,

    pub mode: Mode,
    pub link: Link,
    pub ctx: egui::Context,
    pub pal: Palette,
    /// The desktop's light setting, kept current by `system_theme::spawn`; used only while
    /// `appearance.theme` is "system".
    pub system_light: SystemLight,
    pub snap: Option<Arc<Snapshot>>,
    /// Where the process panel's CPU and memory tints start on this machine (from the snapshot).
    pub heavy: crate::procs::Heavy,
    /// The process panel's action row as drawn this frame (where the kill dialog opens).
    pub actions_rect: Option<egui::Rect>,
    /// Process groups with CPU and GPU shares smoothed (see `Smoother`).
    pub groups: Arc<Vec<ProcGroup>>,
    pub have_procs: bool,
    smoother: Smoother,
    last_proc_unix_ms: Option<u64>,
    seen_counter: u64,

    pub rows: Arc<Vec<Row>>,
    pub rows_dirty: bool,
    pub order: OrderMemory,
    pub last_sort: Instant,
    pub force_resort: bool,
    pub procs_hovered: bool,
    pub expanded: HashSet<String>,
    pub filter: String,
    pub selection: Option<RowId>,
    pub scroll_to_selection: bool,
    pub scroll_offset: f32,
    pub viewport_h: f32,
    pub focus_search: bool,
    /// The search field is open in a panel too narrow to show it all the time.
    pub search_open: bool,
    pub icons: IconCache,

    pub pending_kill: Option<PendingKill>,
    /// The reset waiting for its second click, and since when.
    pub armed: Option<(ResetKind, Instant)>,
    pub kill_watch: Option<KillWatch>,
    /// A text field had the keyboard when the last frame ended.
    typing_last: bool,
    pub alive_check: AliveCheck,
    pub kill_check_for: Duration,
    pub kill_outcome_for: Duration,
    pub toast: Option<Toast>,
    pub kill_tx: Sender<(String, bool)>,
    kill_rx: Receiver<(String, bool)>,
    starting_since: Option<Instant>,
    /// The window has no focus, as last told to the collector.
    pub(crate) sent_unfocused: bool,
    /// Maximize state of the window (the compositor's under Hyprland).
    win: winctl::Controls,
    /// Failed collector start or restart attempts already announced.
    seen_action_failures: u64,

    // Sensor area UI state
    pub edit_layout: bool,
    pub scroll_to_reading: Option<String>,
    pub scroll_to_card: Option<String>,
    /// Where each card's one Tab stop sits (the header unless the arrow keys moved it to a row).
    pub nav: sensors::NavCursor,
    /// Each card's Tab stop of the last frame: widget id, card, and where it sits.
    nav_stops: Vec<(egui::Id, String, Option<sensors::NavKey>)>,
    /// The column the process table's header stop is on (Left and Right move it, Enter sorts).
    pub hdr_cursor: SortKey,
    /// Where each card was drawn last frame (for the legend and the tests).
    pub card_rects: Vec<(String, egui::Rect)>,
    /// Cards whose header was pinned at the top of the view last frame.
    pub sticky_heads: Vec<String>,
    /// The process list area of the last frame.
    pub procs_list_rect: egui::Rect,
    /// x of the Name column's divider in the table header last frame (for the tests).
    pub name_divider_x: f32,
    /// Autofitted widths of the process table's data columns, and the pass that last measured them.
    pub col_fit: crate::procs::ColFit,
    pub col_fit_pass: u64,
    pub hover_device: Option<Device>,
    /// Reading whose history popover is pinned.
    pub pinned: Option<String>,
    /// Row of the pinned reading, when it is on screen.
    pub pinned_hit: Option<Hit>,
    /// Row hovered long enough to show its popover.
    pub hover_pop: Option<Hit>,
    hover_candidate: Option<(String, Instant)>,
    pop_rect: Option<egui::Rect>,
    /// Reading whose popover was drawn last frame.
    pop_id: Option<String>,
    /// Focus request waiting for the first snapshot.
    pub pending_focus: Option<String>,
    /// Reading that flashes after a focus request, and when it started.
    pub flash: Option<(String, Instant)>,
    // Settings window UI state
    pub settings_open: bool,
    /// Settings was closed last frame (it opens under the gear again).
    pub(crate) settings_was_open: bool,
    /// The gear button of the top bar, where Settings opens beneath.
    pub gear_rect: Option<egui::Rect>,
    /// Where the Sources chip was drawn last frame (for the tests).
    pub sources_rect: Option<egui::Rect>,
    pub settings_tab: SettingsTab,
    pub bar_search: String,
    pub interval_edit: Option<u32>,
    /// Settings, Sensors: the search that adds a threshold, and the reading it picked.
    pub thr_search: String,
    pub thr_pick: Option<String>,
    /// Limit changes sent to the collector that the snapshots may not show yet.
    pub thr_pending: Pending,
    /// The error of thresholds.json last announced, so a toast comes once per error.
    limits_error_seen: Option<String>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, mode: Mode, opts: Opts) -> Self {
        Self::with_ctx(&cc.egui_ctx, cc.storage, mode, opts)
    }

    /// Build the app on an egui context (also used by the offscreen tests).
    pub fn with_ctx(ctx: &egui::Context, storage: Option<&dyn eframe::Storage>, mode: Mode, opts: Opts) -> Self {
        let ctx = ctx.clone();
        let settings_path = opts.settings_path.clone().unwrap_or_else(settings::default_path);
        let loaded = settings::load(&settings_path);
        let mut s = loaded.settings;
        let mut migrated = false;
        if !loaded.existed {
            if let Some(old) = storage.and_then(|st| eframe::get_value::<LegacyState>(st, LEGACY_STORAGE_KEY)) {
                migrate_legacy(&mut s, old);
                migrated = true;
            }
        }
        s.appearance.font_size = s.appearance.font_size.clamp(10.0, 24.0);
        theme::set_font_size(s.appearance.font_size);
        let font_choice = FontChoice::parse(&s.appearance.font);
        fonts::install(&ctx, font_choice);
        theme::apply_text_styles(&ctx);
        let system_light: SystemLight = Arc::new(AtomicBool::new(false));
        if !opts.no_watchers {
            // Ask once before the first frame so a light desktop does not flash dark, then follow.
            if ThemeChoice::parse(&s.appearance.theme) == ThemeChoice::System {
                system_light.store(system_theme::read_now() == theme::Scheme::Light, Ordering::Relaxed);
            }
            system_theme::spawn(ctx.clone(), system_light.clone());
        }
        let scheme = ThemeChoice::parse(&s.appearance.theme).resolve(system_theme::scheme_of(&system_light));
        let pal = Palette::for_scheme(scheme);
        Palette::apply(scheme, &ctx);
        ctx.options_mut(|o| o.zoom_with_keyboard = false);
        egui_extras::install_image_loaders(&ctx);

        let external_change = Arc::new(AtomicBool::new(false));
        if !opts.no_watchers {
            settings::spawn_watcher(ctx.clone(), settings_path.clone(), external_change.clone());
        }

        let link = Link::new();
        link.set_procs_wanted(s.layout.process_panel_visible); // a hidden panel does not subscribe to processes
        match &mode {
            Mode::Live => conn::spawn_socket(ctx.clone(), link.clone()),
            Mode::Demo => crate::demo::spawn(ctx.clone(), link.clone()),
            Mode::Mock(path) => {
                match std::fs::read_to_string(path).map_err(|e| e.to_string()).and_then(|t| conn::parse_mock(&t)) {
                    Ok(snap) => {
                        link.lock().never_stale = true;
                        link.publish(&ctx, snap);
                    }
                    Err(e) => eprintln!("hwglance: cannot load mock {}: {e}", path.display()),
                }
            }
        }

        let corrupt_notice = loaded.corrupt.then(|| CorruptNotice { bad: bad_copy_path(&settings_path), saved: false });
        let (kill_tx, kill_rx) = channel();
        let applied_font_size = s.appearance.font_size;
        App {
            s_saved: if migrated { Settings::default() } else { s.clone() },
            s_seen: s.clone(),
            s,
            settings_path,
            last_written: None,
            dirty_since: migrated.then(Instant::now),
            corrupt: loaded.corrupt,
            corrupt_notice,
            external_change,
            save_error: None,
            applied_font_size,
            applied_font: font_choice,
            mode,
            link,
            ctx,
            pal,
            system_light,
            snap: None,
            heavy: crate::procs::Heavy::default(),
            actions_rect: None,
            groups: Arc::new(Vec::new()),
            have_procs: false,
            smoother: Smoother::default(),
            last_proc_unix_ms: None,
            seen_counter: 0,
            rows: Arc::new(Vec::new()),
            rows_dirty: true,
            order: OrderMemory::default(),
            last_sort: Instant::now(),
            force_resort: true,
            procs_hovered: false,
            expanded: HashSet::new(),
            filter: String::new(),
            selection: None,
            scroll_to_selection: false,
            scroll_offset: 0.0,
            viewport_h: 400.0,
            focus_search: false,
            search_open: false,
            icons: IconCache::new(),
            pending_kill: None,
            armed: None,
            kill_watch: None,
            typing_last: false,
            alive_check: actions::is_running,
            kill_check_for: KILL_CHECK_FOR,
            kill_outcome_for: KILL_OUTCOME_FOR,
            toast: None,
            kill_tx,
            kill_rx,
            starting_since: None,
            sent_unfocused: false,
            win: winctl::Controls::new(!opts.no_watchers),
            seen_action_failures: 0,
            edit_layout: false,
            scroll_to_reading: None,
            scroll_to_card: None,
            nav: HashMap::new(),
            nav_stops: Vec::new(),
            hdr_cursor: SortKey::Name,
            card_rects: Vec::new(),
            sticky_heads: Vec::new(),
            procs_list_rect: egui::Rect::NOTHING,
            name_divider_x: 0.0,
            col_fit: Default::default(),
            col_fit_pass: u64::MAX,
            hover_device: None,
            pinned: None,
            pinned_hit: None,
            hover_pop: None,
            hover_candidate: None,
            pop_rect: None,
            pop_id: None,
            pending_focus: opts.focus.clone(),
            flash: None,
            settings_open: opts.open_settings,
            settings_was_open: false,
            gear_rect: None,
            sources_rect: None,
            settings_tab: SettingsTab::General,
            bar_search: String::new(),
            interval_edit: None,
            thr_search: String::new(),
            thr_pick: None,
            thr_pending: Pending::default(),
            limits_error_seen: None,
        }
    }

    // ------------------------------------------------------------------
    // Settings file
    // ------------------------------------------------------------------

    fn settings_tick(&mut self, ctx: &egui::Context) {
        // Changed on disk by someone else (our own writes are recognised by their mtime).
        if self.external_change.swap(false, Ordering::Relaxed) {
            let m = settings::mtime(&self.settings_path);
            if m != self.last_written {
                let loaded = settings::load(&self.settings_path);
                if loaded.existed && !loaded.corrupt && loaded.settings != self.s {
                    self.s = loaded.settings;
                    self.s_saved = self.s.clone();
                    self.s_seen = self.s.clone();
                    self.dirty_since = None;
                    self.rows_dirty = true;
                }
            }
        }
        if self.s != self.s_seen {
            self.s_seen = self.s.clone();
            self.dirty_since = Some(Instant::now());
        }
        if let Some(t) = self.dirty_since {
            let now = Instant::now();
            let due = t + SAVE_DEBOUNCE;
            if now >= due {
                self.dirty_since = None;
                if self.s != self.s_saved {
                    match settings::save_atomic(&self.settings_path, &self.s, self.corrupt) {
                        Ok(()) => {
                            if let (true, Some(n)) = (self.corrupt, &mut self.corrupt_notice) {
                                n.saved = n.bad.exists();
                            }
                            self.corrupt = false;
                            self.s_saved = self.s.clone();
                            self.last_written = settings::mtime(&self.settings_path);
                            if self.save_error.take().is_some() {
                                self.set_toast("Settings saved".into(), true);
                            }
                        }
                        Err(e) => {
                            // Keep what the user changed in memory and try again: the banner says
                            // why, and it stays until a save goes through.
                            self.save_error = Some(settings::save_error_text(&e, &self.settings_path));
                            self.dirty_since = Some(now + SAVE_RETRY - SAVE_DEBOUNCE);
                        }
                    }
                }
            } else {
                ctx.request_repaint_after(due - now + Duration::from_millis(20));
            }
        }
        let scheme = ThemeChoice::parse(&self.s.appearance.theme).resolve(system_theme::scheme_of(&self.system_light));
        if scheme != self.pal.scheme {
            self.pal = Palette::for_scheme(scheme);
            Palette::apply(scheme, ctx);
            ctx.request_repaint();
        }
        let choice = FontChoice::parse(&self.s.appearance.font);
        if choice != self.applied_font {
            self.applied_font = choice;
            fonts::install(ctx, choice);
        }
        let size = self.s.appearance.font_size.clamp(10.0, 24.0);
        if (size - self.applied_font_size).abs() > f32::EPSILON {
            self.applied_font_size = size;
            theme::set_font_size(size);
            theme::apply_text_styles(ctx);
        }
    }

    /// Write pending changes now (on exit).
    pub fn flush_settings(&mut self) {
        if self.s == self.s_saved {
            return;
        }
        match settings::save_atomic(&self.settings_path, &self.s, self.corrupt) {
            Ok(()) => self.s_saved = self.s.clone(),
            Err(e) => eprintln!("hwglance: settings not saved on exit: {}", settings::save_error_text(&e, &self.settings_path)),
        }
    }

    // ------------------------------------------------------------------
    // Data sync
    // ------------------------------------------------------------------

    fn sync(&mut self, ctx: &egui::Context) {
        let (snap, counter) = {
            let s = self.link.lock();
            (s.snapshot.clone(), s.counter)
        };
        if counter != self.seen_counter {
            self.seen_counter = counter;
            if let Some(s) = snap {
                if let Some(procs) = &s.procs {
                    let mut groups = procs.clone();
                    let dt = self
                        .last_proc_unix_ms
                        .map(|prev| (s.unix_ms.saturating_sub(prev)) as f32 / 1000.0)
                        .unwrap_or(1.0)
                        .clamp(0.05, 10.0);
                    self.last_proc_unix_ms = Some(s.unix_ms);
                    self.smoother.apply(&mut groups, dt, self.s.processes.smoothing_seconds);
                    self.groups = Arc::new(groups);
                    self.have_procs = true;
                    self.rows_dirty = true;
                }
                self.heavy = crate::procs::Heavy::of_snapshot(&s);
                // A GPU card that NVML's failure took away comes back empty, to say why in.
                self.snap = Some(sensors::with_placeholder_cards(&s).map(Arc::new).unwrap_or(s));
                self.thr_pending.expire(self.snap.as_deref());
                self.announce_limits_error();
            }
        }
        let (failures, failure) = {
            let s = self.link.lock();
            (s.action_failures, s.action_error.clone())
        };
        if failures != self.seen_action_failures {
            self.seen_action_failures = failures;
            if let Some(text) = failure {
                self.set_toast(text, false);
            }
        }
        while let Ok((text, ok)) = self.kill_rx.try_recv() {
            // polkit answered: the check starts now, or the attempt failed
            if let Some(w) = self.kill_watch.as_mut().filter(|w| w.state == KillState::Waiting) {
                w.since = Instant::now();
                if ok {
                    w.state = KillState::Checking;
                } else {
                    w.state = KillState::Failed;
                    w.message = Some(text.clone());
                }
            }
            self.set_toast(text, ok);
        }
        if let Some(id) = self.link.take_focus() {
            self.apply_focus(ctx, &id, true);
        }
        if self.snap.is_some() {
            if let Some(id) = self.pending_focus.take() {
                self.apply_focus(ctx, &id, false);
            }
        }
        if self.rows_dirty {
            self.rebuild_rows();
        }
    }

    /// Bring a reading into view: expand its card and section, scroll to it, pin its history
    /// popover and flash the row once. `raise` also asks the window system to focus the window.
    pub fn apply_focus(&mut self, ctx: &egui::Context, id: &str, raise: bool) {
        let Some(snap) = self.snap.clone() else {
            self.pending_focus = Some(id.to_string());
            return;
        };
        if !sensors::reveal(&mut self.s, &snap, id) {
            self.set_toast(format!("{} is not reported right now", reading_name(&snap, id)), false);
            return;
        }
        self.scroll_to_reading = Some(id.to_string());
        self.pinned = Some(id.to_string());
        self.pinned_hit = None;
        self.flash = Some((id.to_string(), Instant::now()));
        if raise {
            // A click on an alert asks for the reading's history. The layout editor switches the
            // popover off, so it would raise the window and show nothing but a flash.
            self.edit_layout = false;
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        ctx.request_repaint();
    }

    /// Bring a reading into view without pinning its popover: expand its card and section,
    /// scroll to it and flash the row once (a flagged legend chip does this).
    pub fn jump_to_reading(&mut self, id: &str) {
        let Some(snap) = self.snap.clone() else { return };
        if sensors::reveal(&mut self.s, &snap, id) {
            self.scroll_to_reading = Some(id.to_string());
            self.flash = Some((id.to_string(), Instant::now()));
            self.ctx.request_repaint();
        }
    }

    /// Seconds since the focus flash started, while it is still running.
    fn flash_age(&mut self) -> Option<(String, f32)> {
        let (id, at) = self.flash.clone()?;
        let age = at.elapsed().as_secs_f32();
        if age >= 1.25 {
            self.flash = None;
            return None;
        }
        Some((id, age))
    }

    pub fn groups(&self) -> &[ProcGroup] {
        &self.groups
    }

    pub fn sort_key(&self) -> SortKey {
        SortKey::parse(&self.s.processes.sort.column).unwrap_or(SortKey::Cpu)
    }

    /// Whether the order is being held right now because the pointer is over the list.
    pub fn order_held_by_hover(&self) -> bool {
        self.s.processes.hold_order_on_hover && self.procs_hovered && self.have_procs
    }

    pub fn rebuild_rows(&mut self) {
        let want_hold = self.order_held_by_hover();
        // a damaged or hand-edited value must never reach Duration (it panics on infinity)
        let secs = self.s.processes.resort_seconds;
        let interval = Duration::from_secs_f32(if secs.is_finite() { secs.clamp(0.0, 300.0) } else { 3.0 });
        let due = self.last_sort.elapsed() >= interval;
        let hold = !self.force_resort && !self.order.is_empty() && (want_hold || !due);
        let before = self.selection.as_ref().and_then(|id| model::row_index(&self.rows, id));
        let collapsed: BTreeMap<String, bool> = self.s.processes.collapsed_sections.clone();
        let rows = model::build_rows(
            &self.groups,
            &ViewParams {
                sort: self.sort_key(),
                desc: self.s.processes.sort.descending,
                filter: &self.filter,
                expanded: &self.expanded,
                section_collapsed: &collapsed,
            },
            &mut self.order,
            hold,
        );
        if !hold {
            self.last_sort = Instant::now();
            self.force_resort = false;
        }
        self.rows = Arc::new(rows);
        self.rows_dirty = false;
        // Keep the selected row on screen when the list moved under it.
        let after = self.selection.as_ref().and_then(|id| model::row_index(&self.rows, id));
        if after.is_some() && before != after {
            self.scroll_to_selection = true;
        }
    }

    pub fn set_toast(&mut self, text: String, ok: bool) {
        self.toast = Some(Toast { text, ok, until: Instant::now() + Duration::from_secs(6) });
    }

    pub fn set_sort(&mut self, key: SortKey) {
        let cur = self.sort_key();
        let desc = if cur == key { !self.s.processes.sort.descending } else { key.default_desc() };
        self.s.processes.sort = settings::SortPref { column: key.as_str().into(), descending: desc };
        self.rows_dirty = true;
        self.force_resort = true;
    }

    fn stale(&self) -> bool {
        let s = self.link.lock();
        if s.never_stale {
            return false;
        }
        match (&s.snapshot, s.received) {
            (Some(snap), Some(at)) => at.elapsed() > Duration::from_millis(snap.interval_ms as u64 * 3),
            _ => false,
        }
    }

    // ------------------------------------------------------------------
    // Selection and actions
    // ------------------------------------------------------------------

    pub fn selected_target(&self) -> Option<model::Target> {
        self.selection.as_ref().and_then(|id| model::target_for(self.groups(), id))
    }

    /// Start ending the current selection. Force kill and critical targets confirm first (the dialog
    /// opens with Cancel focused, so Enter cancels).
    pub fn request_kill(&mut self, id: Option<RowId>, signal: Signal) {
        let Some(row) = id.or_else(|| self.selection.clone()) else {
            self.set_toast("Select a process or group first".into(), false);
            return;
        };
        // polkit's prompt for this very row is still open: a second click would stack another one
        if self.kill_watch.as_ref().is_some_and(|w| w.state == KillState::Waiting && w.row == row) {
            self.set_toast("Still waiting for the authentication prompt".into(), false);
            return;
        }
        let Some(target) = model::target_for(self.groups(), &row) else {
            // the process ended between the click and now, or the row is a section
            self.set_toast("That process is gone. Select another one".into(), false);
            return;
        };
        let warning = model::critical_warning(&target);
        if signal == Signal::Kill || warning.is_some() {
            self.pending_kill = Some(PendingKill { row, signal, target, warning, focus_set: false });
        } else {
            self.do_kill(&target, signal, row);
        }
    }

    fn do_kill(&mut self, target: &model::Target, signal: Signal, row: RowId) {
        let out = actions::execute(target, signal, self.kill_tx.clone(), self.ctx.clone());
        self.set_toast(out.message, out.ok);
        // Then look at the target for a few seconds and say how it went in the action row.
        self.kill_watch = Some(KillWatch {
            row,
            name: target.name.clone(),
            signal,
            procs: target.procs.clone(),
            state: if out.waiting_auth { KillState::Waiting } else { KillState::Checking },
            since: Instant::now(),
            message: None,
        });
        self.ctx.request_repaint();
    }

    /// Follow the processes of the last kill: ended, still running after the check window, or
    /// forgotten when the selection moves on or the outcome has been shown long enough.
    fn tick_kill_watch(&mut self, ctx: &egui::Context) {
        let alive = self.alive_check;
        let (check_for, show_for) = (self.kill_check_for, self.kill_outcome_for);
        let Some(w) = self.kill_watch.as_mut() else { return };
        if self.selection.as_ref() != Some(&w.row) {
            self.kill_watch = None;
            return;
        }
        match w.state {
            KillState::Waiting => {}
            KillState::Checking => {
                if !w.procs.iter().any(alive) {
                    w.state = KillState::Ended;
                    w.since = Instant::now();
                } else if w.since.elapsed() >= check_for {
                    w.state = KillState::StillRunning;
                    w.since = Instant::now();
                }
                ctx.request_repaint_after(Duration::from_millis(150));
            }
            KillState::Ended | KillState::StillRunning | KillState::Failed => {
                let shown = w.since.elapsed();
                if shown >= show_for {
                    self.kill_watch = None;
                } else {
                    ctx.request_repaint_after(show_for - shown + Duration::from_millis(30));
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // Two-step resets
    // ------------------------------------------------------------------

    pub fn is_armed(&self, kind: ResetKind) -> bool {
        self.armed.is_some_and(|(k, _)| k == kind)
    }

    /// Act on a click of a two-step reset button.
    pub fn reset_step(&mut self, kind: ResetKind, step: ConfirmStep) {
        match step {
            ConfirmStep::Arm => self.armed = Some((kind, Instant::now())),
            ConfirmStep::Cancel => self.armed = None,
            ConfirmStep::Confirm => {
                self.armed = None;
                match kind {
                    ResetKind::Stats => self.reset_stats(),
                    ResetKind::EditLayout | ResetKind::LayoutAuto => layout::reset(&mut self.s.layout),
                    ResetKind::ProcViews => self.s.processes.views = settings::ProcViews::default(),
                }
            }
        }
    }

    /// An armed reset goes when its button is no longer on screen or it has waited long enough,
    /// so a click much later never lands on a question nobody remembers.
    fn expire_armed(&mut self, ctx: &egui::Context) {
        let Some((kind, at)) = self.armed else { return };
        let on_screen = match kind {
            ResetKind::Stats => self.snap.is_some(),
            ResetKind::EditLayout => self.edit_layout,
            ResetKind::LayoutAuto => self.settings_open && self.settings_tab == SettingsTab::Layout,
            ResetKind::ProcViews => self.settings_open && self.settings_tab == SettingsTab::Processes,
        };
        if !on_screen || at.elapsed() >= ARMED_FOR {
            self.armed = None;
        } else {
            ctx.request_repaint_after(ARMED_FOR - at.elapsed() + Duration::from_millis(30));
        }
    }

    /// What the top bar's Reset clears, as it reads in its question.
    pub fn stats_reset_what(&self) -> String {
        match &self.snap {
            Some(s) => format!("low, high, average, throttle history and crossings since {}", ui_kit::fmt_clock(s.stats_since_unix_ms)),
            None => "low, high, average, throttle history and crossings".to_string(),
        }
    }

    pub fn move_selection(&mut self, delta: isize) {
        if self.rows.is_empty() {
            return;
        }
        let cur = self.selection.as_ref().and_then(|id| model::row_index(&self.rows, id));
        let idx = match cur {
            Some(i) => (i as isize + delta).clamp(0, self.rows.len() as isize - 1) as usize,
            None => {
                if delta >= 0 {
                    0
                } else {
                    self.rows.len() - 1
                }
            }
        };
        self.selection = Some(self.rows[idx].id.clone());
        self.scroll_to_selection = true;
    }

    pub fn toggle_row(&mut self, id: &RowId) {
        match id {
            RowId::Section(c) => {
                let cur = model::section_is_collapsed(&self.s.processes.collapsed_sections, *c);
                self.s.processes.collapsed_sections.insert(model::class_key(*c).into(), !cur);
            }
            RowId::Group(g) => {
                if !self.expanded.remove(g) {
                    self.expanded.insert(g.clone());
                }
            }
            RowId::Proc(..) => return,
        }
        self.rows_dirty = true;
    }

    fn set_expanded(&mut self, id: &RowId, expand: bool) -> bool {
        let Some(row) = model::row_index(&self.rows, id).map(|i| self.rows[i].clone()) else { return false };
        match row.kind {
            model::RowKind::Section { collapsed, .. } if collapsed == expand => self.toggle_row(id),
            model::RowKind::Group { expanded, expandable, .. } if expandable && expanded != expand => self.toggle_row(id),
            _ => return false,
        }
        true
    }

    fn arrow_left(&mut self) {
        let Some(id) = self.selection.clone() else { return };
        if self.set_expanded(&id, false) {
            return;
        }
        // On a child, Left jumps to its group.
        if let Some(i) = model::row_index(&self.rows, &id) {
            if let model::RowKind::Proc { gi, .. } = self.rows[i].kind {
                let gid = self.groups()[gi].id.clone();
                self.selection = Some(RowId::Group(gid));
                self.scroll_to_selection = true;
            }
        }
    }

    fn arrow_right(&mut self) {
        let Some(id) = self.selection.clone() else { return };
        if self.set_expanded(&id, true) {
            return;
        }
        // Already expanded: step into the first child.
        if let Some(i) = model::row_index(&self.rows, &id) {
            if matches!(self.rows[i].kind, model::RowKind::Group { expanded: true, .. }) {
                self.move_selection(1);
            }
        }
    }

    // ------------------------------------------------------------------
    // Keyboard
    // ------------------------------------------------------------------

    fn handle_keys(&mut self, ctx: &egui::Context) {
        // Font size works everywhere, even in the search box. Ctrl+R never resets on the spot: it
        // arms the same confirm as the Reset button, and not at all while a text field has focus
        // or a dialog is open.
        // egui drops the focus of a text field at the start of the pass in which Esc arrives, so
        // "a field is being edited" also counts a field that had the keyboard a frame ago.
        // (`egui_wants_keyboard_input` is true for any focused widget, a button after Tab included;
        // what stops the shortcuts is a text field being edited.)
        let typing_now = ctx.text_edit_focused();
        let editing = typing_now || self.typing_last;
        let (bigger, smaller, reset_size, find, toggle_view) = ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::COMMAND, Key::Equals) || i.consume_key(Modifiers::COMMAND, Key::Plus),
                i.consume_key(Modifiers::COMMAND, Key::Minus),
                i.consume_key(Modifiers::COMMAND, Key::Num0),
                i.consume_key(Modifiers::COMMAND, Key::F),
                i.consume_key(Modifiers::COMMAND, Key::E),
            )
        });
        let ask_reset = !editing && self.pending_kill.is_none() && ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::R));
        if toggle_view {
            // Ctrl+E: the other view (compact or expanded) of the panel's dock
            self.s.toggle_panel_view();
        }
        let fsz = &mut self.s.appearance.font_size;
        if reset_size {
            *fsz = settings::DEFAULT_FONT_SIZE;
        } else if bigger {
            *fsz = (*fsz + 0.5).min(24.0);
        } else if smaller {
            *fsz = (*fsz - 0.5).max(10.0);
        }
        if ask_reset && self.snap.is_some() {
            self.reset_step(ResetKind::Stats, ConfirmStep::Arm);
        }

        // Modal confirmation: Esc cancels. Cancel has the focus when the dialog opens, for a force
        // kill and for ending a critical task alike, so Enter presses Cancel and nothing is
        // confirmed from the keyboard by accident: Force kill or End task takes a click, or Tab
        // then Enter.
        if self.pending_kill.is_some() {
            if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
                self.pending_kill = None;
            }
            return;
        }

        // Esc drops an armed reset.
        if self.armed.is_some() && !editing && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
            self.armed = None;
            return;
        }

        // Esc lets go of a pinned history popover.
        if self.pinned.is_some() && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
            self.pinned = None;
            self.pinned_hit = None;
            return;
        }

        // Esc closes Settings, unless a text field is being edited or a menu or drop-down is open
        // (those take the key first).
        if self.settings_open && !editing && !egui::Popup::is_any_open(ctx) && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
            self.settings_open = false;
            return;
        }

        let search_id = egui::Id::new(SEARCH_ID);
        if find {
            self.focus_search = true;
        }
        if typing_now {
            // In the search box: Esc clears it then leaves; Enter or Down hands focus to the list.
            let has_search_focus = ctx.memory(|m| m.has_focus(search_id));
            if has_search_focus {
                let (esc, down) = ctx.input_mut(|i| {
                    (i.consume_key(Modifiers::NONE, Key::Escape), i.consume_key(Modifiers::NONE, Key::ArrowDown))
                });
                let enter = ctx.input(|i| i.key_pressed(Key::Enter));
                if esc {
                    if self.filter.is_empty() {
                        ctx.memory_mut(|m| m.surrender_focus(search_id));
                    } else {
                        self.filter.clear();
                        self.rows_dirty = true;
                    }
                } else if down || enter {
                    ctx.memory_mut(|m| m.surrender_focus(search_id));
                    if self.selection.is_none() {
                        self.move_selection(1);
                    }
                }
            }
            return;
        }

        let slash = ctx.input_mut(|i| {
            let hit = i.consume_key(Modifiers::NONE, Key::Slash);
            if hit {
                i.events.retain(|e| !matches!(e, Event::Text(t) if t == "/"));
            }
            hit
        });
        if slash {
            self.focus_search = true;
            if !self.s.layout.process_panel_visible {
                self.set_procs_visible(true);
            }
        }
        // A card's Tab stop and the table header own the arrow keys (and Delete) while they have
        // the keyboard, so a key that walks a card never also moves the process selection.
        if let Some(id) = ctx.memory(|m| m.focused())
            && (self.sensor_keys(ctx, id) || id == egui::Id::new(crate::procs::HEADER_ID))
        {
            return;
        }
        if !self.s.layout.process_panel_visible {
            return;
        }
        // The process keys (arrows, Delete, Esc) act when nothing has the keyboard or the list does;
        // on a button, a checkbox or the like they are left to it (Delete must never end a task
        // because Tab happened to stop on Hide), and an open menu has the keys.
        let list_id = egui::Id::new(crate::procs::LIST_ID);
        if ctx.memory(|m| m.focused()).is_some_and(|f| f != list_id) || egui::Popup::is_any_open(ctx) {
            return;
        }
        // Enter on the process list opens or closes the selected group.
        if ctx.memory(|m| m.has_focus(egui::Id::new(crate::procs::LIST_ID)))
            && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter))
            && let Some(id) = self.selection.clone()
        {
            self.toggle_row(&id);
        }
        // Shift+Delete must be consumed before plain Delete, which also matches it.
        let shift_del = ctx.input_mut(|i| i.consume_key(Modifiers::SHIFT, Key::Delete));
        let (up, down, left, right, pgup, pgdn, home, end, del, esc) = ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::NONE, Key::ArrowUp),
                i.consume_key(Modifiers::NONE, Key::ArrowDown),
                i.consume_key(Modifiers::NONE, Key::ArrowLeft),
                i.consume_key(Modifiers::NONE, Key::ArrowRight),
                i.consume_key(Modifiers::NONE, Key::PageUp),
                i.consume_key(Modifiers::NONE, Key::PageDown),
                i.consume_key(Modifiers::NONE, Key::Home),
                i.consume_key(Modifiers::NONE, Key::End),
                i.consume_key(Modifiers::NONE, Key::Delete),
                i.consume_key(Modifiers::NONE, Key::Escape),
            )
        });
        if up {
            self.move_selection(-1);
        }
        if down {
            self.move_selection(1);
        }
        if pgup {
            self.move_selection(-12);
        }
        if pgdn {
            self.move_selection(12);
        }
        if home {
            self.move_selection(-(self.rows.len() as isize));
        }
        if end {
            self.move_selection(self.rows.len() as isize);
        }
        if left {
            self.arrow_left();
        }
        if right {
            self.arrow_right();
        }
        if shift_del {
            self.request_kill(None, Signal::Kill);
        } else if del {
            self.request_kill(None, Signal::Term);
        }
        if esc && self.selection.is_some() {
            self.selection = None;
        }
    }

    /// The keys of a card's Tab stop (its header, or the row the arrows moved it to): Up and Down
    /// walk the card, Home and End go to its ends, Left and Right fold and unfold. Returns whether
    /// `focused` is such a stop.
    fn sensor_keys(&mut self, ctx: &egui::Context, focused: egui::Id) -> bool {
        let Some((_, card, cur)) = self.nav_stops.iter().find(|s| s.0 == focused).cloned() else { return false };
        let Some(snap) = self.snap.clone() else { return true };
        let Some(g) = snap.sensors.iter().find(|g| g.id == card) else { return true };
        let items = sensors::nav_items(g, &self.s);
        let n = items.len() as isize;
        // Esc belongs to an open menu first
        let menu_open = egui::Popup::is_any_open(ctx);
        let (up, down, pgup, pgdn, home, end, left, right, esc) = ctx.input_mut(|i| {
            let k = |i: &mut egui::InputState, key| i.consume_key(Modifiers::NONE, key);
            (
                k(i, Key::ArrowUp),
                k(i, Key::ArrowDown),
                k(i, Key::PageUp),
                k(i, Key::PageDown),
                k(i, Key::Home),
                k(i, Key::End),
                k(i, Key::ArrowLeft),
                k(i, Key::ArrowRight),
                !menu_open && k(i, Key::Escape),
            )
        });
        let delta = if down {
            1
        } else if up {
            -1
        } else if pgdn {
            8
        } else if pgup {
            -8
        } else if home {
            -(n + 1)
        } else if end {
            n + 1
        } else {
            0
        };
        let mut to = cur.clone();
        if delta != 0 {
            to = sensors::nav_step(&items, cur.as_ref(), delta);
        } else if left || right {
            match &cur {
                // fold or unfold what the stop is on; Left on a row goes up to its section
                None => sensors::set_card_collapsed(&mut self.s, &card, left),
                Some(sensors::NavKey::Section(id)) => {
                    self.s.layout.collapsed_sections.insert(id.clone(), left);
                }
                Some(sensors::NavKey::Reading(id)) if left => {
                    to = sensors::section_of(g, id).map(sensors::NavKey::Section);
                }
                Some(sensors::NavKey::Reading(_)) => {}
            }
        }
        if to != cur {
            match &to {
                Some(k) => self.nav.insert(card.clone(), k.clone()),
                None => self.nav.remove(&card),
            };
            // the new stop is focusable this frame (the cursor moved before the cards are laid out)
            ctx.memory_mut(|m| m.request_focus(sensors::nav_id(&card, to.as_ref())));
        }
        if esc {
            ctx.memory_mut(|m| m.surrender_focus(focused));
        }
        true
    }

    /// Tell the collector when the window gains or loses focus: while no window is focused it
    /// samples processes every two seconds instead of every second.
    fn sync_focus(&mut self, ctx: &egui::Context) {
        let unfocused = !ctx.input(|i| i.viewport().focused).unwrap_or(true);
        self.win.tick(ctx, !unfocused);
        if unfocused != self.sent_unfocused {
            self.sent_unfocused = unfocused;
            self.link.set_unfocused(unfocused);
            self.link.send(&ClientMsg::SetProcs { enabled: self.s.layout.process_panel_visible, unfocused });
        }
    }

    pub fn set_procs_visible(&mut self, on: bool) {
        self.s.layout.process_panel_visible = on;
        self.link.send(&ClientMsg::SetProcs { enabled: on, unfocused: self.sent_unfocused });
    }

    /// Ask the collector to change limits; it keeps them in thresholds.json. What was asked shows
    /// at once (see `Pending`), the snapshots confirm it a moment later.
    pub fn send_threshold_edits(&mut self, edits: Vec<ThrEdit>) {
        for e in edits {
            let sent = self.link.send(&e.message());
            if sent || !matches!(self.mode, Mode::Live) {
                self.thr_pending.record(e);
            } else {
                self.set_toast("Collector not connected, cannot change limits".into(), false);
            }
        }
    }

    /// A toast when thresholds.json turns out to be invalid (once per error), and when it is fine again.
    fn announce_limits_error(&mut self) {
        let now = self.snap.as_ref().and_then(|s| s.limits_error.clone());
        if now == self.limits_error_seen {
            return;
        }
        match &now {
            Some(e) => self.set_toast(format!("thresholds.json has an error ({e}); the last good limits stay in force"), false),
            None => self.set_toast("thresholds.json is valid again".into(), true),
        }
        self.limits_error_seen = now;
    }

    pub fn reset_stats(&mut self) {
        if self.link.send(&ClientMsg::ResetStats) {
            self.set_toast("Min, max and average reset".into(), true);
        } else if matches!(self.mode, Mode::Live) {
            self.set_toast("Collector not connected, cannot reset".into(), false);
        }
    }

    // ------------------------------------------------------------------
    // UI
    // ------------------------------------------------------------------

    pub fn show(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        self.sync(&ctx);
        self.sync_focus(&ctx);
        self.handle_keys(&ctx);
        let stale = self.stale();

        self.actions_rect = None;
        self.top_bar(ui, stale);
        self.corrupt_banner(ui);
        self.save_banner(ui);
        self.banner(ui);
        self.version_banner(ui);
        self.limits_banner(ui);
        self.status_bar(ui, stale);

        let pal = self.pal.clone();
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(pal.ground).inner_margin(Margin::same(12)))
            .show(ui, |ui| {
                // Stale data keeps its full contrast (red must stay readable): the "stale" pill
                // and the "as of" stamp in the status bar say it is old.
                self.content(ui);
            });

        self.draw_popover(&ctx);
        self.settings_window(&ctx);
        self.kill_modal(&ctx);
        self.tick_kill_watch(&ctx);
        self.expire_armed(&ctx);
        self.settings_tick(&ctx);
        self.typing_last = ctx.text_edit_focused();
        self.schedule_repaints(&ctx, stale);
    }

    fn content(&mut self, ui: &mut Ui) {
        let full = ui.available_rect_before_wrap();
        let Some(snap) = self.snap.clone() else {
            self.waiting_content(ui, full);
            return;
        };
        let show_procs = self.s.layout.process_panel_visible && self.have_procs;
        if !show_procs {
            self.sensor_area(ui, &snap, full);
            return;
        }
        if self.s.layout.dock() == Dock::Right {
            self.content_docked_right(ui, &snap, full);
            return;
        }
        let handle_h = 12.0;
        let total = full.height();
        let min_h = crate::procs::min_panel_height(crate::procs::MIN_PANEL_ROWS);
        let panel_h = panel_height(self.s.current_view().size.unwrap_or(0.3) * total, total, handle_h, min_h);
        let sensors_rect = egui::Rect::from_min_max(full.min, pos2(full.right(), full.bottom() - panel_h - handle_h));
        let handle_rect = egui::Rect::from_min_max(
            pos2(full.left(), sensors_rect.bottom()),
            pos2(full.right(), sensors_rect.bottom() + handle_h),
        );
        let panel_rect = egui::Rect::from_min_max(pos2(full.left(), handle_rect.bottom()), full.max);

        self.sensor_area(ui, &snap, sensors_rect);

        ui.scope_builder(egui::UiBuilder::new().max_rect(panel_rect), |ui| {
            self.procs_panel(ui);
        });

        // The split handle is 12 px of layout with a thin grip, and 24 px of hit area: 6 px more
        // above and below. It is made after both neighbours so it wins those pixels.
        let hit = handle_rect.expand2(vec2(0.0, (theme::MIN_HIT - handle_h) / 2.0));
        let resp = ui.interact(hit, egui::Id::new("split-handle"), Sense::click_and_drag());
        let spoken_h = panel_h;
        a11y::publish(&resp, || {
            Spoken::new(egui::accesskit::Role::Splitter, "Resize the process panel")
                .described("Up and Down move the divider by 4 pixels, with Shift by 32. Home restores the default height. Double-click or Ctrl+E switches between compact and expanded.")
                .valued(format!("{} pixels high", spoken_h.round()))
        });
        if resp.hovered() || resp.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
        }
        let key = ui_kit::divider_key(&resp, false);
        if resp.double_clicked() {
            // one click away: the other view of this dock
            self.s.toggle_panel_view();
        } else if key == Some(ui_kit::DividerKey::Reset) {
            self.s.reset_panel_size();
        } else if total > 0.0 {
            // a drag or an arrow key resizes the current view only; the panel is below the divider,
            // so the divider moving down makes it shorter
            let drag = if resp.dragged() { ui_kit::drag_delta(ui, &resp).y } else { 0.0 };
            let step = if let Some(ui_kit::DividerKey::Move(d)) = key { d } else { 0.0 };
            if drag != 0.0 || step != 0.0 {
                let new_h = panel_height(panel_h - drag - step, total, handle_h, min_h);
                self.s.current_view_mut().size = Some((new_h / total).clamp(0.1, 0.85));
            }
        }
        if resp.has_focus() {
            a11y::paint_ring(ui.painter(), handle_rect, 4.0, a11y::Ring::Inside, self.pal.ink);
        }
        let color = if resp.hovered() || resp.dragged() || resp.has_focus() { self.pal.ink3 } else { self.pal.rule };
        ui.painter().rect_filled(egui::Rect::from_center_size(handle_rect.center(), vec2(64.0, 3.0)), 2.0, color);
    }

    /// Where the panels will be, while there is no snapshot. A collector that is not running
    /// says so, gives the reason and carries the action to start it. A running collector that
    /// has not sent its first snapshot shows a skeleton in the saved layout's columns.
    fn waiting_content(&mut self, ui: &mut Ui, full: egui::Rect) {
        let pal = self.pal.clone();
        let (connected, failed, reason, action_error, unreadable) = {
            let s = self.link.lock();
            (s.connected, s.connect_failed, s.last_error.clone(), s.action_error.clone(), s.unreadable)
        };
        let state = waiting_state(matches!(self.mode, Mode::Live), connected, failed);
        // The process panel's place, so the layout does not jump when the data arrives.
        let mut area = full;
        if state != Waiting::NotRunning {
            self.starting_since = None;
            if self.s.layout.process_panel_visible {
                let (sensors, panel) = self.waiting_split(ui, full);
                area = sensors;
                crate::procs::skeleton_panel(ui.painter(), panel, &pal);
            }
            ui.scope_builder(egui::UiBuilder::new().max_rect(area), |ui| {
                // placeholders taller than the room stop at the panel instead of covering it
                ui.set_clip_rect(area.intersect(ui.clip_rect()));
                sensors::skeleton(ui, &pal, area.width(), &self.s.layout);
            });
        }
        // A failed Start collector adds its reason (a few lines) under the usual one.
        let extra = action_error.as_ref().map_or(0.0, |e| fs(20.0) * (e.chars().count() as f32 / 52.0).ceil().max(1.0) + fs(8.0));
        let unreadable = unreadable && state == Waiting::FirstSnapshot;
        let size = if state == Waiting::NotRunning {
            vec2(fs(460.0), fs(124.0) + extra)
        } else if unreadable {
            vec2(fs(460.0), fs(110.0))
        } else {
            vec2(fs(360.0), fs(56.0))
        };
        let c = egui::Rect::from_center_size(area.center(), size);
        ui.painter().rect(c, 8.0, pal.panel, Stroke::new(1.0, pal.rule), egui::StrokeKind::Inside);
        match state {
            Waiting::NotRunning => {
                ui.scope_builder(
                    egui::UiBuilder::new().max_rect(c.shrink(16.0)).layout(Layout::top_down(Align::Center)),
                    |ui| {
                        ui.label(RichText::new("Collector not running").font(f_title()).color(pal.crit));
                        ui.add_space(4.0);
                        let why = reason.unwrap_or_else(|| "Nothing answers on the collector's socket.".to_string());
                        ui.label(RichText::new(why).color(pal.ink2));
                        if let Some(e) = &action_error {
                            ui.add_space(4.0);
                            ui.label(RichText::new(e).color(pal.crit));
                        }
                        ui.add_space(8.0);
                        self.start_collector_button(ui, false);
                    },
                );
            }
            Waiting::FirstSnapshot if unreadable => {
                ui.scope_builder(
                    egui::UiBuilder::new().max_rect(c.shrink(16.0)).layout(Layout::top_down(Align::Center)),
                    |ui| {
                        ui.label(RichText::new("Collector data not readable").font(f_title()).color(pal.crit));
                        ui.add_space(4.0);
                        ui.label(RichText::new("The collector is running but sends data this window cannot read, most likely another version.").color(pal.ink2));
                        ui.add_space(8.0);
                        if ui.button("Restart collector").on_hover_text("systemctl --user restart hwglanced").clicked() {
                            conn::restart_collector(&self.link, ui.ctx());
                            self.set_toast("Restarting the collector".into(), true);
                        }
                    },
                );
            }
            Waiting::FirstSnapshot | Waiting::Connecting => {
                let text = if state == Waiting::FirstSnapshot { "Waiting for the first snapshot..." } else { "Connecting to the collector..." };
                ui.scope_builder(
                    egui::UiBuilder::new().max_rect(c).layout(Layout::centered_and_justified(egui::Direction::TopDown)),
                    |ui| {
                        ui.label(RichText::new(text).font(f_label_bold()).color(pal.ink2));
                    },
                );
            }
        }
    }

    /// Where the sensors and the process panel go in `full`, for the placeholder: the same split
    /// `content` makes, without the divider.
    fn waiting_split(&self, ui: &Ui, full: egui::Rect) -> (egui::Rect, egui::Rect) {
        let handle = 12.0;
        let view = self.s.current_view().clone();
        if self.s.layout.dock() == Dock::Right {
            let compact = self.s.layout.view() == settings::PanelView::Compact;
            let w = if compact {
                let min = crate::procs::compact_min_width(ui, &self.col_fit, &view.columns);
                dock_right_width_px(view.width_px.unwrap_or(settings::COMPACT_WIDTH_PX), min, full.width(), handle)
            } else {
                dock_right_width(view.size.unwrap_or(0.55), full.width(), handle)
            };
            let panel = egui::Rect::from_min_max(pos2(full.right() - w, full.top()), full.max);
            (egui::Rect::from_min_max(full.min, pos2(panel.left() - handle, full.bottom())), panel)
        } else {
            let min_h = crate::procs::min_panel_height(crate::procs::MIN_PANEL_ROWS);
            let h = panel_height(view.size.unwrap_or(0.3) * full.height(), full.height(), handle, min_h);
            let panel = egui::Rect::from_min_max(pos2(full.left(), full.bottom() - h), full.max);
            (egui::Rect::from_min_max(full.min, pos2(full.right(), panel.top() - handle)), panel)
        }
    }

    /// "Start collector" (disabled as "Starting..." for a few seconds after a click).
    fn start_collector_button(&mut self, ui: &mut Ui, small: bool) {
        let starting = self.starting_since.is_some_and(|t| t.elapsed() < Duration::from_secs(4));
        ui.add_enabled_ui(!starting, |ui| {
            let label = if starting { "Starting..." } else { "Start collector" };
            let btn = if small { ui.small_button(label) } else { ui.button(label) };
            if btn.on_hover_text("systemctl --user start hwglanced").clicked() {
                conn::start_collector(&self.link, ui.ctx());
                self.starting_since = Some(Instant::now());
                ui.ctx().request_repaint_after(Duration::from_secs(4));
            }
        });
    }

    /// The process panel at the right, full height, behind a draggable vertical divider.
    fn content_docked_right(&mut self, ui: &mut Ui, snap: &Arc<Snapshot>, full: egui::Rect) {
        let handle_w = 12.0;
        let compact = self.s.layout.view() == settings::PanelView::Compact;
        let view = self.s.current_view().clone();
        // the compact view never gets narrower than where all its columns show
        if compact {
            self.refresh_col_fit(ui, &crate::model::visible_columns(&view.columns));
        }
        let min_compact_w = if compact { crate::procs::compact_min_width(ui, &self.col_fit, &view.columns) } else { 0.0 };
        let panel_w = if compact {
            dock_right_width_px(view.width_px.unwrap_or(settings::COMPACT_WIDTH_PX), min_compact_w, full.width(), handle_w)
        } else {
            dock_right_width(view.size.unwrap_or(0.55), full.width(), handle_w)
        };
        let panel_rect = egui::Rect::from_min_max(pos2(full.right() - panel_w, full.top()), full.max);
        let handle_rect = egui::Rect::from_min_max(pos2(panel_rect.left() - handle_w, full.top()), pos2(panel_rect.left(), full.bottom()));
        let sensors_rect = egui::Rect::from_min_max(full.min, pos2(handle_rect.left(), full.bottom()));

        self.sensor_area(ui, snap, sensors_rect);

        ui.scope_builder(egui::UiBuilder::new().max_rect(panel_rect), |ui| {
            self.procs_panel(ui);
        });

        // 12 px of layout, 24 px of hit area: the extra 12 px lie over the panel's own left padding
        // (not toward the sensors, whose scroll bar sits against the handle). Made after the
        // panel so it wins them.
        let hit = egui::Rect::from_min_max(handle_rect.min, pos2(handle_rect.right() + (theme::MIN_HIT - handle_w), handle_rect.bottom()));
        let resp = ui.interact(hit, egui::Id::new("split-handle-right"), Sense::click_and_drag());
        a11y::publish(&resp, || {
            Spoken::new(egui::accesskit::Role::Splitter, "Resize the process panel")
                .described("Left and Right move the divider by 4 pixels, with Shift by 32. Home restores the default width. Double-click or Ctrl+E switches between compact and expanded.")
                .valued(format!("{} pixels wide", panel_w.round()))
        });
        if resp.hovered() || resp.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }
        let key = ui_kit::divider_key(&resp, true);
        if resp.double_clicked() {
            self.s.toggle_panel_view();
        } else if key == Some(ui_kit::DividerKey::Reset) {
            self.s.reset_panel_size();
        } else if full.width() > 0.0 {
            // a drag or an arrow key resizes the current view only: pixels for the compact one, a
            // share otherwise; the panel is right of the divider, so the divider moving right
            // makes it narrower
            let drag = if resp.dragged() { ui_kit::drag_delta(ui, &resp).x } else { 0.0 };
            let step = if let Some(ui_kit::DividerKey::Move(d)) = key { d } else { 0.0 };
            if drag != 0.0 || step != 0.0 {
                if compact {
                    let new_w = dock_right_width_px(panel_w - drag - step, min_compact_w, full.width(), handle_w);
                    self.s.current_view_mut().width_px = Some(new_w);
                } else {
                    let new_w = dock_right_width((panel_w - drag - step) / full.width(), full.width(), handle_w);
                    self.s.current_view_mut().size = Some((new_w / full.width()).clamp(0.2, 0.85));
                }
            }
        }
        if resp.has_focus() {
            a11y::paint_ring(ui.painter(), handle_rect, 4.0, a11y::Ring::Inside, self.pal.ink);
        }
        let color = if resp.hovered() || resp.dragged() || resp.has_focus() { self.pal.ink3 } else { self.pal.rule };
        ui.painter().rect_filled(egui::Rect::from_center_size(handle_rect.center(), vec2(3.0, 64.0)), 2.0, color);
    }

    fn sensor_area(&mut self, ui: &mut Ui, snap: &Arc<Snapshot>, rect: egui::Rect) {
        let pal = self.pal.clone();
        let mut scroll_to = self.scroll_to_reading.take();
        let mut scroll_to_card = self.scroll_to_card.take();
        let mut copy: Option<String> = None;
        let edit = self.edit_layout;
        let flash = self.flash_age();
        let pinned = self.pinned.clone();
        let hover_device = self.hover_device;
        // hovering a row opens the popover (and not the tooltip) unless editing or a dialog is up
        let popover = !edit && self.pending_kill.is_none();
        let reset_armed = self.is_armed(ResetKind::EditLayout);
        let mut out = ViewOut::default();
        ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
            ui.set_clip_rect(rect.intersect(ui.clip_rect()));
            let avail = rect.width() - ui.spacing().scroll.allocated_width();
            egui::ScrollArea::both().id_salt("sensors-scroll").auto_shrink([false, false]).show(ui, |ui| {
                let mut view = sensors::SensorView {
                    snap,
                    pal: &pal,
                    s: &mut self.s,
                    edit,
                    scroll_to: &mut scroll_to,
                    scroll_to_card: &mut scroll_to_card,
                    hover_device,
                    copy: &mut copy,
                    flash: flash.as_ref().map(|(id, age)| (id.as_str(), *age)),
                    pinned: pinned.as_deref(),
                    popover,
                    reset_armed,
                    nav: &mut self.nav,
                    pending: &self.thr_pending,
                    out: &mut out,
                };
                sensors::show(ui, &mut view, avail);
            });
        });
        if flash.is_some() {
            ui.ctx().request_repaint();
        }
        if out.stop_editing {
            self.edit_layout = false;
        }
        if let Some(step) = out.reset_layout {
            self.reset_step(ResetKind::EditLayout, step);
        }
        self.scroll_to_reading = scroll_to;
        self.scroll_to_card = scroll_to_card;
        if let Some(text) = copy {
            ui.ctx().copy_text(text);
            self.set_toast("Value copied".into(), true);
        }
        self.send_threshold_edits(std::mem::take(&mut out.thr_edits));
        self.card_rects = out.card_rects.clone();
        self.sticky_heads = out.sticky.clone();
        self.nav_stops = std::mem::take(&mut out.stops);
        self.update_popover(ui.ctx(), out);
    }

    /// Hover and click bookkeeping of the history popover.
    fn update_popover(&mut self, ctx: &egui::Context, out: ViewOut) {
        if let Some(h) = out.clicked {
            self.pinned = Some(h.id);
        }
        if let Some(h) = out.show_history {
            self.pinned = Some(h.id.clone());
            self.pinned_hit = Some(h);
        }
        // A click somewhere else (not on a row, not on the popover) lets go of the pin.
        let pressed = ctx.input(|i| i.pointer.primary_pressed());
        if pressed && self.pinned.is_some() && !self.pop_hit_by_pointer(ctx) {
            let on_row = ctx.input(|i| i.pointer.interact_pos()).is_some_and(|p| {
                out.pinned.as_ref().is_some_and(|h| h.rect.contains(p))
                    || out.hover.as_ref().is_some_and(|h| h.rect.contains(p))
            });
            if !on_row {
                self.pinned = None;
                self.pinned_hit = None;
            }
        }
        if let Some(h) = out.pinned {
            if self.pinned.as_deref() == Some(h.id.as_str()) {
                self.pinned_hit = Some(h);
            }
        }
        // Hover for 350 ms opens the popover.
        match out.hover {
            Some(h) => {
                let same = self.hover_candidate.as_ref().is_some_and(|(id, _)| *id == h.id);
                if same {
                    let waited = self.hover_candidate.as_ref().map(|(_, t)| t.elapsed()).unwrap_or_default();
                    if waited >= HOVER_DELAY {
                        self.hover_pop = Some(h);
                    } else {
                        ctx.request_repaint_after(HOVER_DELAY - waited + Duration::from_millis(10));
                    }
                } else {
                    self.hover_candidate = Some((h.id.clone(), Instant::now()));
                    self.hover_pop = None;
                    ctx.request_repaint_after(HOVER_DELAY + Duration::from_millis(10));
                }
            }
            None => {
                self.hover_candidate = None;
                self.hover_pop = None;
            }
        }
    }

    fn pop_hit_by_pointer(&self, ctx: &egui::Context) -> bool {
        match (self.pop_rect, ctx.input(|i| i.pointer.interact_pos())) {
            (Some(r), Some(p)) => r.contains(p),
            _ => false,
        }
    }

    pub fn history_minutes(&self) -> u32 {
        let from_snap = self.snap.as_ref().map(|s| s.history_minutes).unwrap_or(0);
        if from_snap > 0 { from_snap } else { self.s.sensors.history_minutes.max(1) }
    }

    /// The popover beside a hovered or pinned row: history of that reading.
    fn draw_popover(&mut self, ctx: &egui::Context) {
        self.pop_rect = None;
        if self.edit_layout || self.pending_kill.is_some() {
            self.pop_id = None;
            return;
        }
        let pinned = self.pinned.is_some();
        let hit = if pinned { self.pinned_hit.clone() } else { self.hover_pop.clone() };
        let Some(hit) = hit else {
            self.pop_id = None;
            return;
        };
        let minutes = self.history_minutes();
        // Fresh data every 2 s while it is open; right after opening, a copy younger than 5 s
        // is good enough.
        let opened_now = self.pop_id.as_deref() != Some(hit.id.as_str());
        let every = if opened_now && self.link.history_of(&hit.id).is_some_and(|(_, age)| age < Duration::from_secs(5)) {
            Duration::from_secs(5)
        } else {
            Duration::from_secs(2)
        };
        self.link.request_history(&hit.id, minutes * 60, every);
        self.pop_id = Some(hit.id.clone());
        ctx.request_repaint_after(Duration::from_secs(2));
        let series = self.link.history_of(&hit.id).map(|(s, _)| s);
        let now_ms = self.snap.as_ref().map(|s| s.unix_ms).unwrap_or(0);
        let screen = ctx.content_rect();
        let w = history::POP_W;
        let pal = self.pal.clone();
        // What the row's tooltip would say goes under the plot.
        let mut notes: Vec<(String, Color32)> = Vec::new();
        // The onset, always: since when it is past a threshold, or its most recent crossing.
        let crossing = self.snap.as_ref().and_then(|s| s.crossings.iter().rfind(|c| c.id == hit.id).cloned());
        if let Some((text, level)) = history::onset_note(series.as_deref(), crossing.as_ref(), hit.unit, &hit.thr, now_ms, u64::from(minutes) * 60_000) {
            notes.push((text, level.and_then(|l| pal.level_color(l)).unwrap_or(pal.ink2)));
        }
        if let Some(d) = &hit.desc {
            notes.push((d.clone(), pal.ink2));
        }
        if let Some(n) = &hit.note {
            notes.push((format!("Source: {n}"), pal.ink3));
        }
        if let Some(r) = &hit.range {
            notes.push((r.clone(), pal.ink3));
        }
        let notes_h = if notes.is_empty() {
            0.0
        } else {
            let lines: f32 = notes.iter().map(|(t, c)| ctx.fonts_mut(|f| f.layout(t.clone(), f_small(), *c, w - 24.0)).size().y).sum();
            4.0 + lines + 4.0 * (notes.len() as f32 - 1.0)
        };
        let h = history::POP_H + notes_h;
        // beside or under the row, wherever it covers the least of the other cards
        let own = self.card_rects.iter().find(|(_, r)| r.contains(hit.rect.center())).map(|(id, _)| id.clone());
        let mut avoid: Vec<egui::Rect> = self.card_rects.iter().filter(|(id, _)| Some(id) != own.as_ref()).map(|(_, r)| *r).collect();
        if self.s.layout.process_panel_visible && self.have_procs {
            avoid.push(self.procs_list_rect);
        }
        let pos = popover_pos(hit.rect, vec2(w, h), screen, &avoid);
        let input = PopInput {
            color: hit.color,
            title: hit.title.clone(),
            unit: hit.unit,
            value: hit.value,
            thr: hit.thr,
            series: series.as_deref(),
            now_ms,
            history_minutes: minutes,
            crossing,
        };
        let area = egui::Area::new(egui::Id::new("history-popover"))
            .order(egui::Order::Tooltip)
            .fixed_pos(pos)
            .interactable(pinned)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(pal.panel)
                    .stroke(Stroke::new(1.0, pal.rule))
                    .corner_radius(8.0)
                    .shadow(pal.floating_shadow())
                    .inner_margin(Margin::same(12))
                    .show(ui, |ui| {
                        ui.set_max_width(w - 24.0);
                        let (rect, plot) = ui.allocate_exact_size(vec2(w - 24.0, history::POP_H - 24.0), Sense::hover());
                        a11y::publish(&plot, || Spoken::new(egui::accesskit::Role::Image, history::spoken_summary(&input)));
                        history::paint(ui.painter(), rect, &pal, &input);
                        if !notes.is_empty() {
                            ui.add_space(4.0 - ui.spacing().item_spacing.y);
                            for (t, c) in &notes {
                                ui.add(egui::Label::new(RichText::new(t).font(f_small()).color(*c)));
                            }
                        }
                    });
            });
        self.pop_rect = Some(area.response.rect);
    }

    fn top_bar(&mut self, ui: &mut Ui, stale: bool) {
        let pal = self.pal.clone();
        self.hover_device = None;
        let present: Vec<(Device, String, DeviceState)> = {
            let mut v: Vec<(Device, String, DeviceState)> = Vec::new();
            if let Some(snap) = &self.snap {
                for dev in [Device::Cpu, Device::Gpu, Device::Igpu, Device::Memory, Device::Drive, Device::Board] {
                    if let Some(g) = snap.sensors.iter().find(|g| Device::of_card(&g.id) == dev && !self.s.sensors.hidden_cards.contains(&g.id)) {
                        v.push((dev, g.id.clone(), sensors::device_state(snap, &self.s, dev)));
                    }
                }
            }
            v
        };
        let mut jump: Option<String> = None;
        let mut jump_pinned: Option<String> = None;
        // What ran hot and since when: the collector's crossings for the readings on screen.
        let snap_now = self.snap.clone();
        let hidden_cards = self.s.sensors.hidden_cards.clone();
        let cross: Vec<crossings::Entry> =
            snap_now.as_ref().map(|s| crossings::entries(s, |id| hidden_cards.iter().any(|h| h == id))).unwrap_or_default();
        let now_ms = snap_now.as_ref().map_or(0, |s| s.unix_ms);
        let window_ms = u64::from(self.history_minutes()) * 60_000;
        egui::Panel::top("top-bar")
            .frame(egui::Frame::new().fill(pal.ground).inner_margin(Margin::symmetric(16, 8)))
            .show_separator_line(false)
            .show(ui, |ui| {
                // Fit the bar to its width before anything can overlap: first the "Since" text
                // goes, then the Sources chip shrinks to its dot, then the legend names shorten
                // and finally become dots (their names stay in the tooltips).
                let (density, widths) = {
                    let measure = |text: &str, font: FontId| ui.painter().layout_no_wrap(text.to_string(), font, pal.ink2).size().x;
                    let pad = 2.0 * ui.spacing().button_padding.x;
                    let space = ui.spacing().item_spacing.x;
                    let chips = |form: LegendForm| -> f32 {
                        present.iter().map(|(dev, _, st)| legend_chip_width(measure(form.text(*dev), f_label()), st.mark.map(|_| 14.0), form) + space).sum()
                    };
                    let sources = self.snap.as_ref().map(|_| sources_chip_width(measure("Sources 4 of 4", f_label()), SourcesForm::Text) + space);
                    // the crossings chip: its words while the legend has names, its count after
                    let cross_w = |form: crossings::ChipForm| {
                        if cross.is_empty() { 0.0 } else { crossings::chip_width(measure(&crossings::chip_label(&cross, form), f_label())) + space + 8.0 }
                    };
                    let widths = BarWidths {
                        left: [
                            measure("HW Glance", f_title()) + 12.0 + chips(LegendForm::Full) + cross_w(crossings::ChipForm::Text),
                            measure("HW Glance", f_title()) + 12.0 + chips(LegendForm::Short) + cross_w(crossings::ChipForm::Text),
                            measure("HW Glance", f_title()) + 12.0 + chips(LegendForm::Dot) + cross_w(crossings::ChipForm::Count),
                        ],
                        // gear, maximize and close, Edit layout, Reset (the short form), the stale pill when it is up
                        right_fixed: measure(&icon::COG.to_string(), ui_kit::f_icon(base() + 2.0))
                            + pad
                            + winctl::group_width(measure(&icon::COG.to_string(), ui_kit::f_icon(base() + 2.0)) + pad)
                            + measure("Edit layout", f_label())
                            + pad
                            + measure("Reset", f_label())
                            + pad
                            + if stale { 64.0 } else { 0.0 }
                            + 4.0 * space,
                        sources: sources.map(|w| [w, sources_chip_width(0.0, SourcesForm::Dot) + space]),
                        since: self.snap.as_ref().map(|s| measure(&format!("Since {}", ui_kit::fmt_clock(s.stats_since_unix_ms)), f_label()) + space),
                    };
                    (bar_density(ui.available_width(), &widths), widths)
                };
                // room for the Reset button's long question: what is left of the bar after the
                // legend side and the other right-hand parts (measured above)
                let reset_room = {
                    let sources = widths.sources.map_or(0.0, |s| s[if density.sources() == SourcesForm::Text { 0 } else { 1 }]);
                    let left = widths.left[match density.legend() {
                        LegendForm::Full => 0,
                        LegendForm::Short => 1,
                        LegendForm::Dot => 2,
                    }];
                    ui.available_width() - left - widths.right_fixed - sources - 120.0
                };
                // A double-click on an empty part of the bar maximizes or restores, as on a title
                // bar. It is made first, so every control drawn over it wins its own clicks, and
                // it is no Tab stop (the buttons are).
                let bar = ui.interact(ui.max_rect().expand2(vec2(16.0, 8.0)), egui::Id::new("top-bar-empty"), Sense::CLICK);
                if bar.double_clicked() {
                    self.win.toggle_maximize(ui.ctx());
                }
                ui.horizontal(|ui| {
                    ui.label(RichText::new("HW Glance").font(f_title()).color(pal.ink));
                    ui.add_space(12.0);
                    for (dev, card, state) in &present {
                        let recent = crossings::recent_level(&cross, Some(*dev), now_ms, window_ms);
                        let history = crossings::latest_recent(&cross, *dev, now_ms, window_ms).map(|e| crossings::sentence(&e, now_ms));
                        let resp = legend_chip(ui, &pal, *dev, state, density.legend(), recent, history.as_deref());
                        if resp.hovered() {
                            self.hover_device = Some(*dev);
                        }
                        if resp.clicked() {
                            // A flagged chip goes to the flagged row, the others to their panel.
                            match &state.target {
                                Some(id) => jump = Some(id.clone()),
                                None => self.scroll_to_card = Some(card.clone()),
                            }
                        }
                    }
                    if !cross.is_empty() {
                        ui.add_space(8.0);
                        let form = if density.legend() == LegendForm::Dot { crossings::ChipForm::Count } else { crossings::ChipForm::Text };
                        let tone = crossings::recent_level(&cross, None, now_ms, window_ms);
                        let tip = format!("{}. Click for the list.", crossings::chip_text(&cross));
                        let chip = crossings::chip(ui, &pal, &cross, tone, form, &tip);
                        let picked = egui::Popup::from_toggle_button_response(&chip)
                            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                            .show(|ui| crossings::list(ui, &pal, &cross, now_ms, window_ms))
                            .and_then(|r| r.inner);
                        if picked.is_some() {
                            jump_pinned = picked;
                        }
                    }

                    // Left to right, so Tab goes Reset, Sources, Edit layout, Settings, Maximize, Close.
                    ui_kit::right_cluster(ui, |ui| {
                        if let (true, Some(s)) = (density.since(), &self.snap) {
                            ui.label(RichText::new(format!("Since {}", ui_kit::fmt_clock(s.stats_since_unix_ms))).color(pal.ink3));
                        }
                        let what = self.stats_reset_what();
                        let armed = self.is_armed(ResetKind::Stats);
                        // The question names what goes; a narrow bar keeps the short form
                        // (the long one is in the tooltip).
                        let long = format!("Click again to clear {what}");
                        let wide = ui.painter().layout_no_wrap(long.clone(), f_label(), pal.crit).size().x + 24.0 < reset_room;
                        let c = Confirm {
                            enabled: self.snap.is_some(),
                            label: "Reset",
                            armed_label: if wide { &long } else { "Click again to clear history" },
                            tip: &format!("Clear {what} (Ctrl+R). Asks again before it does."),
                            disabled_tip: "Nothing to reset until the collector sends data",
                            rtl: false,
                        };
                        if let Some(step) = confirm_button(ui, &pal, armed, &c) {
                            self.reset_step(ResetKind::Stats, step);
                        }
                        ui.add_space(8.0);
                        if stale {
                            status_pill(ui, "stale", pal.warn, "No fresh snapshot for more than three intervals");
                        }
                        if let Some(s) = self.snap.clone() {
                            // a click opens Settings on the Collector tab, where the sources are listed
                            let chip = sources_chip(ui, &pal, &s, density.sources());
                            self.sources_rect = Some(chip.rect);
                            if chip.clicked() {
                                self.settings_open = true;
                                self.settings_tab = SettingsTab::Collector;
                            }
                        }
                        ui.add_space(8.0);
                        // framed like the other buttons of the bar; pressed while editing
                        let edit = ui
                            .add(egui::Button::new("Edit layout").selected(self.edit_layout))
                            .on_hover_text("Unlock the panels for moving and resizing. Normal clicks never move anything.");
                        if edit.clicked() {
                            self.edit_layout = !self.edit_layout;
                        }
                        let gear = ui.button(RichText::new(icon::COG.to_string()).font(ui_kit::f_icon(base() + 2.0)));
                        a11y::publish(&gear, || Spoken::button("Settings").described("Opens the settings window. Esc closes it.").selected(self.settings_open));
                        let gear = gear.on_hover_text("Settings");
                        self.gear_rect = Some(gear.rect);
                        if gear.clicked() {
                            self.settings_open = !self.settings_open;
                        }
                        let ctx = ui.ctx().clone();
                        match winctl::show_group(ui, &pal, self.win.is_maximized(&ctx), gear.rect.size()) {
                            Some(winctl::Control::Maximize) => self.win.toggle_maximize(&ctx),
                            Some(winctl::Control::Close) => winctl::close(&ctx),
                            None => {}
                        }
                    });
                });
            });
        if let Some(id) = jump {
            self.jump_to_reading(&id);
        }
        // A row of the crossings list scrolls to its reading, flashes it and pins its history.
        if let Some(id) = jump_pinned {
            let ctx = ui.ctx().clone();
            self.apply_focus(&ctx, &id, false);
        }
    }

    /// The one-time notice that ui.json was unreadable and names the copy kept of it.
    fn corrupt_banner(&mut self, ui: &mut Ui) {
        let Some(notice) = self.corrupt_notice.clone() else { return };
        let pal = self.pal.clone();
        let home = std::env::var("HOME").unwrap_or_default();
        let mut dismiss = false;
        egui::Panel::top("settings-banner")
            .frame(egui::Frame::new().fill(pal.warn.gamma_multiply(0.14)).inner_margin(Margin::symmetric(16, 6)))
            .show_separator_line(false)
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("Settings reset").font(f_label_bold()).color(pal.ink));
                    ui.label(RichText::new(notice.text(&home)).color(pal.ink));
                    if ui.button("Dismiss").on_hover_text("Hide this notice").clicked() {
                        dismiss = true;
                    }
                });
            });
        if dismiss {
            self.corrupt_notice = None;
        }
    }

    /// ui.json cannot be written (a full disk, a read-only folder): what the user changed lasts
    /// until the window closes, and the window keeps trying. Shown until a save goes through.
    fn save_banner(&mut self, ui: &mut Ui) {
        let Some(why) = self.save_error.clone() else { return };
        let pal = self.pal.clone();
        let mut retry = false;
        egui::Panel::top("save-banner")
            .frame(egui::Frame::new().fill(pal.warn.gamma_multiply(0.14)).inner_margin(Margin::symmetric(16, 6)))
            .show_separator_line(false)
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("Settings not saved").font(f_label_bold()).color(pal.ink));
                    ui.label(RichText::new(format!("Your changes work for now but are lost when the window closes: {why}. HW Glance tries again every few seconds.")).color(pal.ink));
                    if ui.button("Try again").on_hover_text("Save the settings now").clicked() {
                        retry = true;
                    }
                });
            });
        if retry {
            let now = Instant::now();
            self.dirty_since = Some(now.checked_sub(SAVE_DEBOUNCE).unwrap_or(now));
            ui.ctx().request_repaint();
        }
    }

    fn banner(&mut self, ui: &mut Ui) {
        // Without a snapshot the centre of the window carries the message and the action.
        if !matches!(self.mode, Mode::Live) || self.snap.is_none() {
            return;
        }
        let down = {
            let s = self.link.lock();
            !s.connected && s.connect_failed
        };
        if !down {
            self.starting_since = None;
            return;
        }
        let pal = self.pal.clone();
        let (banner_fill, reason_ink) = pal.crit_banner();
        egui::Panel::top("collector-banner")
            .frame(egui::Frame::new().fill(banner_fill).inner_margin(Margin::symmetric(16, 6)))
            .show_separator_line(false)
            .show(ui, |ui| {
                // wrapped: the reason and a failed start's error are sentences, and a narrow window
                // must show them whole instead of running off its edge
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("Collector not running").font(f_label_bold()).color(pal.crit));
                    self.start_collector_button(ui, true);
                    let (last, action) = {
                        let s = self.link.lock();
                        (s.last_error.clone(), s.action_error.clone())
                    };
                    if let Some(e) = last {
                        ui.label(RichText::new(e).color(reason_ink));
                    }
                    if let Some(e) = action {
                        ui.label(RichText::new(e).color(pal.crit));
                    }
                });
            });
    }

    /// The collector speaks another protocol version than this window: say which way, and what
    /// to do. Only for the live collector (the demo and mock data have none to mismatch).
    fn version_banner(&mut self, ui: &mut Ui) {
        if !matches!(self.mode, Mode::Live) {
            return;
        }
        let Some(note) = self.snap.as_ref().and_then(|s| conn::version_note(s.proto)) else { return };
        let pal = self.pal.clone();
        let mut restart = false;
        egui::Panel::top("collector-version")
            .frame(egui::Frame::new().fill(pal.warn.gamma_multiply(0.14)).inner_margin(Margin::symmetric(16, 6)))
            .show_separator_line(false)
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(if note.older { "Collector is older" } else { "Collector is newer" }).font(f_label_bold()).color(pal.ink));
                    ui.label(RichText::new(&note.text).color(pal.ink));
                    if note.older && ui.button("Restart collector").on_hover_text("systemctl --user restart hwglanced").clicked() {
                        restart = true;
                    }
                });
            });
        if restart {
            conn::restart_collector(&self.link, ui.ctx());
            self.set_toast("Restarting the collector".into(), true);
        }
    }

    /// thresholds.json cannot be used: where, and that the last good limits stay in force. Shown
    /// until the file is fixed (the collector checks it every couple of seconds).
    fn limits_banner(&mut self, ui: &mut Ui) {
        let Some(err) = self.snap.as_ref().and_then(|s| s.limits_error.clone()) else { return };
        let pal = self.pal.clone();
        let mut open = false;
        egui::Panel::top("limits-banner")
            .frame(egui::Frame::new().fill(pal.warn.gamma_multiply(0.14)).inner_margin(Margin::symmetric(16, 6)))
            .show_separator_line(false)
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("Limits file has an error").font(f_label_bold()).color(pal.ink));
                    ui.label(RichText::new(format!("thresholds.json, {err}. The last good limits stay in force and the file is left as it is.")).color(pal.ink));
                    if ui.button("Open file").on_hover_text("Open thresholds.json with your default editor").clicked() {
                        open = true;
                    }
                });
            });
        if open && let Err(e) = actions::open_path(&settings::thresholds_path().display().to_string()) {
            self.set_toast(format!("Could not open the file: {e}"), false);
        }
    }

    fn status_bar(&mut self, ui: &mut Ui, stale: bool) {
        let pal = self.pal.clone();
        egui::Panel::bottom("status-bar")
            .frame(egui::Frame::new().fill(pal.ground).inner_margin(Margin::symmetric(16, 6)))
            .show_separator_line(false)
            .show(ui, |ui| {
                // what the right end says: how old the data is, and a toast while there is one
                let age = {
                    let s = self.link.lock();
                    s.received.map(|t| s.pinned_age.unwrap_or_else(|| t.elapsed().as_secs_f32()))
                };
                let age_str = age.map(|a| age_text(stale, self.snap.as_ref().map(|s| s.unix_ms), a));
                let mut toast = self.toast.as_ref().filter(|t| Instant::now() < t.until).map(|t| (t.text.clone(), t.ok));
                // the left end gives up parts, in order, before it can run into the right end
                let numbers = self.status_numbers();
                let density = {
                    let spacing = ui.spacing().item_spacing.x;
                    let measure = |text: &str, font: FontId| ui.painter().layout_no_wrap(text.to_string(), font, pal.ink2).size().x;
                    let left_of = |form: StatusDensity| -> f32 {
                        let segs = status_segments(form, &numbers);
                        segs.iter().map(|s| measure(&s.text, if s.mono { ui_kit::f_mono() } else { f_label() }) + s.after).sum::<f32>() + spacing * segs.len().saturating_sub(1) as f32
                    };
                    let show_btn = if self.s.layout.process_panel_visible { 0.0 } else { measure(&format!("{}  Processes", icon::EYE), f_label()) + 2.0 * ui.spacing().button_padding.x + spacing };
                    let age_w = age_str.as_ref().map_or(0.0, |t| measure(t, f_label()));
                    let toast_w = toast.as_ref().map_or(0.0, |(t, _)| measure(t, f_label()) + 12.0 + spacing);
                    let left = [StatusDensity::Full, StatusDensity::NoGroups, StatusDensity::ShortMemory, StatusDensity::Bare].map(|f| left_of(f) + show_btn);
                    let mut w = StatusWidths { left, right: age_w + toast_w };
                    // a toast is for a few seconds: in a window too narrow for it beside the numbers
                    // and the age, it waits for room instead of running into them
                    if toast.is_some() && !w.fits_compact(ui.available_width()) {
                        toast = None;
                        w.right = age_w;
                    }
                    status_density(ui.available_width(), &w)
                };
                ui.horizontal(|ui| {
                    if !self.s.layout.process_panel_visible {
                        let b = ui.button(RichText::new(format!("{}  Processes", icon::EYE)).font(f_label()));
                        a11y::publish(&b, || Spoken::button("Show the process panel"));
                        if b.on_hover_text("Show the process panel").clicked() {
                            self.set_procs_visible(true);
                        }
                    }
                    for seg in status_segments(density, &numbers) {
                        let text = if seg.mono { RichText::new(seg.text).font(ui_kit::f_mono()).color(pal.ink2) } else { RichText::new(seg.text).color(pal.ink3) };
                        ui.label(text);
                        ui.add_space(seg.after);
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if let Some(t) = &age_str {
                            let color = if stale { pal.warn } else { pal.ink3 };
                            ui.label(RichText::new(t).color(color));
                        }
                        if let Some((text, ok)) = &toast {
                            let c = if *ok { pal.ink } else { pal.crit };
                            ui.add_space(12.0);
                            ui.label(RichText::new(text).color(c));
                        }
                    });
                });
            });
    }

    /// The numbers of the status bar's left end.
    fn status_numbers(&self) -> StatusNumbers {
        let procs = self.have_procs.then(|| (self.groups.iter().map(|g| g.procs.len()).sum::<usize>(), self.groups.len()));
        let snap = self.snap.as_ref().map(|snap| {
            let s = &snap.summary;
            StatusMachine {
                cpu: opt_pct(s.cpu_usage_pct),
                gpu: opt_pct(s.gpu_usage_pct),
                ram: match (s.ram_used_bytes, s.ram_total_bytes) {
                    (Some(u), Some(t)) if t > 0.0 => Some((u, t, s.ram_used_pct.unwrap_or(u / t * 100.0))),
                    _ => None,
                },
            }
        });
        StatusNumbers { procs, machine: snap }
    }

    fn kill_modal(&mut self, ctx: &egui::Context) {
        let Some(pk) = &self.pending_kill else { return };
        let pal = self.pal.clone();
        let n = pk.target.procs.len();
        let force = pk.signal == Signal::Kill;
        let title = pk.signal.label();
        let name = pk.target.name.clone();
        let warning = pk.warning.clone();
        let mut confirm = false;
        let mut cancel = false;
        let mut focus_cancel = !pk.focus_set;
        let frame = egui::Frame::popup(&ctx.global_style()).inner_margin(Margin::same(16));
        // It opens just above the process panel's action row, whose buttons asked for it (centred
        // when the panel is not on screen), and the dark window gets a heavier scrim.
        let id = egui::Id::new("kill-modal");
        let mut modal = egui::Modal::new(id).frame(frame).backdrop_color(pal.scrim());
        if let Some(r) = self.actions_rect {
            modal = modal.area(
                egui::Area::new(id)
                    .kind(egui::UiKind::Modal)
                    .sense(Sense::hover())
                    .order(egui::Order::Foreground)
                    .interactable(true)
                    .pivot(egui::Align2::LEFT_BOTTOM)
                    .fixed_pos(pos2(r.left(), r.top() - 8.0)),
            );
        }
        let modal = modal.show(ctx, |ui| {
            ui.set_width(fs(440.0));
            ui.label(RichText::new(format!("{title}?")).font(FontId::new(fs(18.0), fonts::bold())).color(pal.ink));
            ui.add_space(6.0);
            ui.label(
                RichText::new(format!("{}  ({n} {})", crate::text::shape(&name), if n == 1 { "process" } else { "processes" }))
                    .strong()
                    .color(pal.ink),
            );
            ui.add_space(4.0);
            ui.label(kill_blurb(force));
            if let Some(w) = &warning {
                ui.add_space(8.0);
                egui::Frame::new()
                    .fill(pal.crit.gamma_multiply(0.18))
                    .stroke(Stroke::new(1.0, pal.crit))
                    .corner_radius(6.0)
                    .inner_margin(Margin::same(8))
                    .show(ui, |ui| {
                        ui.label(RichText::new(w).color(pal.ink));
                    });
            }
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                // Cancel first: it has the focus when the dialog opens, and Tab leads on to the
                // confirm button.
                let cancel_btn = ui.button("Cancel  (Esc)");
                if focus_cancel {
                    cancel_btn.request_focus();
                    focus_cancel = false;
                }
                if cancel_btn.clicked() {
                    cancel = true;
                }
                let (fill, ink) = pal.crit_button();
                let btn = egui::Button::new(RichText::new(title).color(ink).strong()).fill(fill);
                if ui.add(btn).clicked() {
                    confirm = true;
                }
            });
        });
        if let Some(pk) = &mut self.pending_kill {
            pk.focus_set = true;
        }
        if modal.should_close() {
            cancel = true;
        }
        if confirm {
            if let Some(pk) = self.pending_kill.take() {
                self.do_kill(&pk.target, pk.signal, pk.row);
            }
        } else if cancel {
            self.pending_kill = None;
        }
    }

    fn schedule_repaints(&mut self, ctx: &egui::Context, stale: bool) {
        // Wake exactly when the data turns stale; while stale keep the age readout moving.
        if stale {
            ctx.request_repaint_after(Duration::from_secs(1));
        } else {
            let s = self.link.lock();
            if !s.never_stale {
                if let (Some(snap), Some(at)) = (&s.snapshot, s.received) {
                    let limit = Duration::from_millis(snap.interval_ms as u64 * 3);
                    let left = limit.saturating_sub(at.elapsed()) + Duration::from_millis(30);
                    ctx.request_repaint_after(left);
                }
            }
        }
        if let Some(t) = &self.toast {
            let now = Instant::now();
            if now < t.until {
                ctx.request_repaint_after(t.until - now + Duration::from_millis(30));
            } else {
                self.toast = None;
            }
        }
    }
}

fn migrate_legacy(s: &mut Settings, old: LegacyState) {
    s.layout.process_panel_fraction = old.split.clamp(0.1, 0.85);
    s.seed_views_from_legacy();
    s.layout.process_panel_visible = old.procs_visible;
    s.processes.sort.column = old.sort.as_str().into();
    s.processes.sort.descending = old.sort_desc;
    for (id, open) in old.section_open {
        s.layout.collapsed_sections.insert(id, !open);
    }
    for (class, collapsed) in old.proc_sections {
        s.processes.collapsed_sections.insert(class, collapsed);
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        self.show(ui);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.flush_settings();
    }

    fn persist_egui_memory(&self) -> bool {
        false
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        let c = self.pal.ground;
        [c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0, 1.0]
    }
}

/// How a reading is called on screen: "Package" in the CPU panel is "CPU, Package". Falls back
/// to a plain "That value" for a reading the collector does not report.
pub fn reading_name(snap: &Snapshot, id: &str) -> String {
    for g in &snap.sensors {
        for sec in &g.sections {
            if let Some(r) = sec.readings.iter().chain(sec.summary.iter()).find(|r| r.id == id) {
                return format!("{}, {}", Device::of_card(&g.id).short(), r.label);
            }
        }
    }
    "That value".to_string()
}

/// What the kill dialog says the action does. The bar widget's popup uses the same sentences.
pub fn kill_blurb(force: bool) -> &'static str {
    if force {
        "Force kill stops the process at once. It cannot save data or clean up."
    } else {
        "End task asks the process to exit, so it can save data and clean up."
    }
}

/// Where the history popover of a row goes: beside the row (right, then left) or under or over
/// it inside its own card, whichever covers the least of `avoid` (the other cards and the
/// process panel). It never covers the row itself, and it stays on screen. The first of equal
/// choices wins, so a one-column window still gets the popover at the row's right.
pub fn popover_pos(row: egui::Rect, size: egui::Vec2, screen: egui::Rect, avoid: &[egui::Rect]) -> egui::Pos2 {
    let (lo, hi) = (screen.min + vec2(4.0, 4.0), screen.max - size - vec2(4.0, 4.0));
    let clamp = |p: egui::Pos2| pos2(p.x.min(hi.x).max(lo.x), p.y.min(hi.y).max(lo.y));
    let beside_y = row.center().y - 40.0;
    let candidates = [
        pos2(row.right() + 8.0, beside_y),
        pos2(row.left() - 8.0 - size.x, beside_y),
        pos2(row.left() + 16.0, row.bottom() + 4.0),
        pos2(row.left() + 16.0, row.top() - 4.0 - size.y),
    ];
    let overlap = |a: egui::Rect, b: egui::Rect| {
        let i = a.intersect(b);
        if i.is_positive() { i.width() * i.height() } else { 0.0 }
    };
    let mut best: Option<(f32, egui::Pos2)> = None;
    for c in candidates {
        let p = clamp(c);
        let r = egui::Rect::from_min_size(p, size);
        let cost = overlap(r, row) * 1000.0 + avoid.iter().map(|a| overlap(r, *a)).sum::<f32>() + (p - c).length();
        if best.is_none_or(|(b, _)| cost < b) {
            best = Some((cost, p));
        }
    }
    best.map(|(_, p)| p).unwrap_or(candidates[0])
}

/// Least width the process table keeps when docked right, and the sensors keep beside it.
pub const MIN_PANEL_W: f32 = 520.0;
pub const MIN_SENSORS_W: f32 = 480.0;

/// Width of the process panel docked right: its share of the content width, but at least
/// `MIN_PANEL_W` and leaving `MIN_SENSORS_W` (plus the divider) for the sensors. When the window
/// is too narrow for both, the sensors keep their minimum and the panel gets the rest.
pub fn dock_right_width(fraction: f32, total: f32, handle_w: f32) -> f32 {
    let max_w = (total - handle_w - MIN_SENSORS_W).max(0.0);
    let min_w = MIN_PANEL_W.min(max_w);
    (fraction * total).clamp(min_w, max_w)
}

/// Width of the compact right panel, set in logical pixels: clamped between `min_px` (the width at
/// which its columns all show, from `procs::compact_min_width`) and what leaves the sensors their
/// minimum and the divider. A saved width below the minimum just renders at the minimum.
pub fn dock_right_width_px(px: f32, min_px: f32, total: f32, handle_w: f32) -> f32 {
    let max_w = (total - handle_w - MIN_SENSORS_W).max(0.0);
    px.clamp(min_px.min(max_w), max_w)
}

/// Clamp the process panel height so both areas keep a usable minimum. The panel keeps at least
/// `min_rows_h` (its chrome and about five rows) unless that would leave the sensors under 120 px.
pub fn panel_height(want: f32, total: f32, handle_h: f32, min_rows_h: f32) -> f32 {
    let roomy = (total - 120.0 - handle_h).max(0.0);
    let min_panel = min_rows_h.min(roomy).max(170.0_f32.min(total * 0.5));
    let max_panel = roomy.max(min_panel);
    want.clamp(min_panel, max_panel)
}

/// The status bar's freshness note: "updated 0.8 s ago", and once the data is stale "as of 12:31,
/// 47 s ago", because the window no longer dims it and the time is what says it is old.
pub fn age_text(stale: bool, as_of_ms: Option<u64>, age_s: f32) -> String {
    match (stale, as_of_ms) {
        (true, Some(ms)) => format!("as of {}, {age_s:.0} s ago", fmt_clock(ms)),
        _ => format!("updated {age_s:.1} s ago"),
    }
}

fn opt_pct(v: Option<f64>) -> String {
    v.map(|v| format!("{v:.1} %")).unwrap_or_else(|| "n/a".into())
}

pub fn source_label(name: &str) -> String {
    match name.to_ascii_lowercase().as_str() {
        "nvml" => "NVML".into(),
        "lact" => "LACT".into(),
        "helper" => "Helper".into(),
        "hyprland" => "Hyprland".into(),
        other => {
            let mut c = other.chars();
            c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
        }
    }
}

/// Share of the state colour that tints a legend chip while its device is flagged.
pub const LEGEND_NOW_TINT: f32 = 0.07;

/// Legend chip of the top bar: the device colour as an 8 px dot and its name. A device that is
/// flagged right now gets a ring in the state colour and a short mark ("H", "HH", a
/// thermometer for throttling), so the worst state of each device reads from the top bar.
/// How much of its name a legend chip shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LegendForm {
    /// "Memory", "Drives".
    Full,
    /// "Mem", "Drive".
    Short,
    /// Just the device dot (the name is in the tooltip).
    Dot,
}

impl LegendForm {
    pub fn text(self, dev: Device) -> &'static str {
        match (self, dev) {
            (LegendForm::Full, d) => d.legend(),
            (LegendForm::Short, Device::Memory) => "Mem",
            (LegendForm::Short, Device::Drive) => "Drive",
            (LegendForm::Short, d) => d.short(),
            (LegendForm::Dot, _) => "",
        }
    }
}

/// Width of a legend chip: the dot, the name, the insets and the state mark if it has one.
pub fn legend_chip_width(label_w: f32, mark_w: Option<f32>, form: LegendForm) -> f32 {
    let mark = mark_w.map(|m| 6.0 + m).unwrap_or(0.0);
    match form {
        LegendForm::Dot => 10.0 + 8.0 + 10.0 + mark,
        _ => 8.0 + 6.0 + label_w + 20.0 + mark,
    }
}

fn legend_chip(ui: &mut Ui, pal: &Palette, dev: Device, state: &DeviceState, form: LegendForm, recent: Option<theme::Level>, history: Option<&str>) -> egui::Response {
    let g = ui.painter().layout_no_wrap(form.text(dev).to_string(), f_label(), pal.ink2);
    let state_color = pal.level_color(state.level);
    let mark = state.mark.zip(state_color).map(|(m, c)| {
        let font = if m == sensors::THROTTLE_MARK { ui_kit::f_icon(base()) } else { ui_kit::f_flag() };
        (ui.painter().layout_no_wrap(m.to_string(), font, c), c)
    });
    let mark_w = mark.as_ref().map(|(m, _)| m.size().x);
    let size = vec2(legend_chip_width(g.size().x, mark_w, form), snap_h());
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(rect, 6.0, pal.hover);
    }
    // Now: a tinted fill and a ring. A crossing that is recent but over: the hollow ring alone.
    if let Some((c, hollow)) = crossings::ring_for(state.level, recent, pal) {
        if !hollow {
            ui.painter().rect_filled(rect, 6.0, c.gamma_multiply(LEGEND_NOW_TINT));
        }
        ui.painter().rect_stroke(rect, 6.0, Stroke::new(1.5, c), egui::StrokeKind::Inside);
    }
    let color = dev.color(pal);
    ui.painter().circle_filled(pos2(rect.left() + 10.0 + 4.0, rect.center().y), 4.0, color);
    let text_x = rect.left() + 10.0 + 8.0 + 6.0;
    let x = if form == LegendForm::Dot {
        rect.left() + 10.0 + 8.0 + 2.0
    } else {
        ui.painter().galley(pos2(text_x, rect.center().y - g.size().y / 2.0), g.clone(), pal.ink2);
        text_x + g.size().x + 6.0
    };
    if let Some((m, c)) = mark {
        ui.painter().galley(pos2(x, rect.center().y - m.size().y / 2.0), m, c);
    }
    let mut tip = match &state.target {
        Some(_) => format!("{}: {} {}. Click to jump to it.", dev.legend(), state.label, state.meaning()),
        None => format!("Jump to the {} panel", dev.legend()),
    };
    if let Some(h) = history {
        tip.push('\n');
        tip.push_str(h);
    }
    a11y::focus_ring(ui, &resp, 6.0, Ring::Outside, pal);
    a11y::publish(&resp, || Spoken::button(tip.replace('\n', ". ")));
    resp.on_hover_text(tip.clone())
}

/// How much the top bar has given up to fit its width, in order: the "Since" text, the words of
/// the Sources chip, the full names of the legend, the names altogether.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum BarDensity {
    Full,
    NoSince,
    SourcesDot,
    ShortLegend,
    DotLegend,
}

impl BarDensity {
    pub fn since(self) -> bool {
        self < BarDensity::NoSince
    }
    pub fn sources(self) -> SourcesForm {
        if self < BarDensity::SourcesDot { SourcesForm::Text } else { SourcesForm::Dot }
    }
    pub fn legend(self) -> LegendForm {
        match self {
            BarDensity::ShortLegend => LegendForm::Short,
            BarDensity::DotLegend => LegendForm::Dot,
            _ => LegendForm::Full,
        }
    }
}

/// What the top bar's two ends want, measured: the left end (title and legend) with full, short
/// and no legend names, the right end's buttons that never go, and the parts that do.
#[derive(Clone, Copy, Debug)]
pub struct BarWidths {
    pub left: [f32; 3],
    pub right_fixed: f32,
    /// The Sources chip with its words and as a dot, with the gap after it (`None` without data).
    pub sources: Option<[f32; 2]>,
    /// The "Since" text with its gap (`None` without data).
    pub since: Option<f32>,
}

/// Air kept between the legend and the buttons.
const BAR_GAP: f32 = 24.0;

/// The first density whose widths fit `avail`; the most compact when none does.
pub fn bar_density(avail: f32, w: &BarWidths) -> BarDensity {
    let need = |left: f32, sources: usize, since: bool| {
        left + BAR_GAP + w.right_fixed + w.sources.map(|s| s[sources]).unwrap_or(0.0) + if since { w.since.unwrap_or(0.0) } else { 0.0 }
    };
    for (d, n) in [
        (BarDensity::Full, need(w.left[0], 0, true)),
        (BarDensity::NoSince, need(w.left[0], 0, false)),
        (BarDensity::SourcesDot, need(w.left[0], 1, false)),
        (BarDensity::ShortLegend, need(w.left[1], 1, false)),
    ] {
        if n <= avail {
            return d;
        }
    }
    BarDensity::DotLegend
}

fn snap_h() -> f32 {
    theme::hit_h(28.0)
}

/// How much of the Sources chip shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourcesForm {
    Text,
    Dot,
}

pub fn sources_chip_width(text_w: f32, form: SourcesForm) -> f32 {
    match form {
        SourcesForm::Text => text_w + 14.0 + 20.0,
        SourcesForm::Dot => 28.0,
    }
}

/// What the Sources chip says: quiet words while every source answers, "Sources 3 of 4" when
/// one does not.
pub fn sources_text(ok: usize, total: usize) -> String {
    if source_health(ok, total) == SourceHealth::Healthy { "Sources".to_string() } else { format!("Sources {ok} of {total}") }
}

/// The Sources chip: plain grey text and a grey dot while every source answers, so it is not
/// worth a glance; an outlined chip in amber or red with the count when one fails. Details on hover.
fn sources_chip(ui: &mut Ui, pal: &Palette, snap: &Snapshot, form: SourcesForm) -> egui::Response {
    let ok = snap.sources.iter().filter(|s| s.ok).count();
    let total = snap.sources.len();
    let health = source_health(ok, total);
    let healthy = health == SourceHealth::Healthy;
    let color = source_dot_color(health, pal);
    let text = if form == SourcesForm::Text { sources_text(ok, total) } else { String::new() };
    let ink = if healthy { pal.ink3 } else { pal.ink };
    let g = ui.painter().layout_no_wrap(text, f_label(), ink);
    let size = vec2(sources_chip_width(g.size().x, form), snap_h());
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(rect, 14.0, pal.hover);
    }
    if !healthy {
        ui.painter().rect(rect, 14.0, color.gamma_multiply(0.14), Stroke::new(1.5, color), egui::StrokeKind::Inside);
    }
    let dot_x = if form == SourcesForm::Dot { rect.center().x } else { rect.left() + 12.0 };
    ui.painter().circle_filled(pos2(dot_x, rect.center().y), if healthy { 3.0 } else { 4.0 }, color);
    if form == SourcesForm::Text {
        ui.painter().galley(pos2(rect.left() + 22.0, rect.center().y - g.size().y / 2.0), g, ink);
    }
    a11y::focus_ring(ui, &resp, 14.0, Ring::Outside, pal);
    a11y::publish(&resp, || {
        let down: Vec<String> = snap.sources.iter().filter(|s| !s.ok).map(|s| source_label(&s.name)).collect();
        let name = if down.is_empty() { format!("Sources, {ok} of {total} answering") } else { format!("Sources, {ok} of {total} answering, not answering: {}", down.join(", ")) };
        Spoken::button(name).described("Opens Settings, Collector")
    });
    resp.on_hover_ui(|ui| {
        ui.label(RichText::new(format!("Sources: {ok} of {total} answering. Click to open Settings, Collector.")).strong());
        for s in &snap.sources {
            ui.horizontal(|ui| {
                let c = if s.ok { pal.ink3 } else { pal.warn };
                let (r, _) = ui.allocate_exact_size(vec2(8.0, 8.0), Sense::hover());
                ui.painter().circle_filled(r.center(), 4.0, c);
                ui.label(RichText::new(source_label(&s.name)).strong());
                ui.label(RichText::new(&s.detail).color(pal.ink3));
            });
        }
    })
}

/// A small outlined status word (stale).
pub fn status_pill(ui: &mut Ui, text: &str, color: Color32, tip: &str) {
    let g = ui.painter().layout_no_wrap(text.to_string(), f_small(), color);
    let size = g.size() + vec2(16.0, 8.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter().rect(rect, size.y / 2.0, Color32::TRANSPARENT, Stroke::new(1.0, color), egui::StrokeKind::Inside);
    ui.painter().galley(rect.center() - g.size() / 2.0, g, color);
    a11y::publish(&resp, || Spoken::new(egui::accesskit::Role::Status, text).described(tip));
    resp.on_hover_text(tip);
}

/// What the status bar's left end shows: the process count and group count, then the machine's
/// CPU, GPU and memory.
#[derive(Clone, Debug, PartialEq)]
pub struct StatusNumbers {
    pub procs: Option<(usize, usize)>,
    pub machine: Option<StatusMachine>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StatusMachine {
    pub cpu: String,
    pub gpu: String,
    /// Used and total bytes, and the used share in percent.
    pub ram: Option<(f64, f64, f64)>,
}

/// How much the status bar's left end has given up to fit beside its right end, in order: the
/// group count ("in 101 groups"), the total RAM, then the process count and the size of the RAM.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum StatusDensity {
    Full,
    NoGroups,
    ShortMemory,
    Bare,
}

/// One piece of the left end: its text, the face, and the gap that follows it.
#[derive(Clone, Debug, PartialEq)]
pub struct StatusSeg {
    pub text: String,
    pub mono: bool,
    pub after: f32,
}

/// The pieces of the left end at a density: "666 processes in 101 groups  CPU 13.0 %  GPU 39.0 %
/// Memory 25.3 GiB / 62.0 GiB (41 %)" in full, shortening to "CPU 13.0 %  GPU 39.0 %  Memory 41 %".
pub fn status_segments(form: StatusDensity, n: &StatusNumbers) -> Vec<StatusSeg> {
    let mono = |t: String, after: f32| StatusSeg { text: t, mono: true, after };
    let dim = |t: &str| StatusSeg { text: t.to_string(), mono: false, after: 0.0 };
    let mut out = Vec::new();
    if let (Some((procs, groups)), true) = (n.procs, form != StatusDensity::Bare) {
        if form == StatusDensity::Full {
            out.push(mono(format!("{procs} processes"), 0.0));
            out.push(dim("in"));
            out.push(mono(format!("{groups} groups"), 12.0));
        } else {
            out.push(mono(format!("{procs} processes"), 12.0));
        }
    }
    if let Some(m) = &n.machine {
        out.push(dim("CPU"));
        out.push(mono(m.cpu.clone(), 8.0));
        out.push(dim("GPU"));
        out.push(mono(m.gpu.clone(), 8.0));
        out.push(dim("Memory"));
        out.push(mono(
            match (&m.ram, form) {
                (Some((u, t, p)), StatusDensity::Full | StatusDensity::NoGroups) => {
                    format!("{} / {} ({:.0} %)", format_value(Unit::Bytes, *u), format_value(Unit::Bytes, *t), p)
                }
                (Some((u, _, p)), StatusDensity::ShortMemory) => format!("{} ({:.0} %)", format_value(Unit::Bytes, *u), p),
                (Some((_, _, p)), StatusDensity::Bare) => format!("{p:.0} %"),
                (None, _) => "n/a".into(),
            },
            0.0,
        ));
    }
    out
}

/// What the status bar's two ends want, measured: the left end in each density (most to least)
/// and the right end (the age of the data and a toast).
#[derive(Clone, Copy, Debug)]
pub struct StatusWidths {
    pub left: [f32; 4],
    pub right: f32,
}

impl StatusWidths {
    /// Does even the most compact left end fit beside the right end?
    pub fn fits_compact(&self, avail: f32) -> bool {
        self.left[3] + STATUS_GAP + self.right <= avail
    }
}

/// Air kept between the two ends of the status bar.
const STATUS_GAP: f32 = 24.0;

/// The first density whose left end fits beside the right end; the most compact when none does.
pub fn status_density(avail: f32, w: &StatusWidths) -> StatusDensity {
    [StatusDensity::Full, StatusDensity::NoGroups, StatusDensity::ShortMemory, StatusDensity::Bare]
        .into_iter()
        .zip(w.left)
        .find(|(_, left)| left + STATUS_GAP + w.right <= avail)
        .map_or(StatusDensity::Bare, |(d, _)| d)
}

pub use crate::ui_kit::fmt_clock;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dead_collector_is_told_apart_from_one_that_is_just_slow() {
        // live source, the connection failed: say it is not running and offer to start it
        assert_eq!(waiting_state(true, false, true), Waiting::NotRunning);
        // connected, no snapshot yet: skeleton and waiting
        assert_eq!(waiting_state(true, true, false), Waiting::FirstSnapshot);
        assert_eq!(waiting_state(true, true, true), Waiting::FirstSnapshot);
        // first attempt still in flight, or a source that cannot fail (demo, mock)
        assert_eq!(waiting_state(true, false, false), Waiting::Connecting);
        assert_eq!(waiting_state(false, false, true), Waiting::Connecting);
    }

    #[test]
    fn a_flagged_legend_chip_keeps_its_text_readable_on_the_state_tint() {
        for (name, pal, floor) in [("dark", Palette::dark(), 4.0), ("light", Palette::light(), 4.5)] {
            for state in [pal.warn, pal.crit] {
                for (bg, ink) in [(pal.ground, pal.ink2), (pal.hover, pal.ink2), (pal.ground, state)] {
                    let tinted = bg.lerp_to_gamma(state, LEGEND_NOW_TINT);
                    assert!(ui_kit::contrast(ink, tinted) >= floor, "{name}: {:.2}", ui_kit::contrast(ink, tinted));
                }
            }
        }
    }

    #[test]
    fn the_status_bar_says_as_of_when_the_data_is_stale() {
        let t = 1_700_000_000_000;
        assert_eq!(age_text(false, Some(t), 0.84), "updated 0.8 s ago");
        let stale = age_text(true, Some(t), 47.2);
        assert_eq!(stale, format!("as of {}, 47 s ago", fmt_clock(t)));
        // without a snapshot time it falls back to the plain readout
        assert_eq!(age_text(true, None, 12.0), "updated 12.0 s ago");
    }

    #[test]
    fn the_sources_dot_is_never_green() {
        let pal = Palette::dark();
        assert_eq!(source_health(4, 4), SourceHealth::Healthy);
        assert_eq!(source_health(0, 0), SourceHealth::Healthy);
        assert_eq!(source_health(3, 4), SourceHealth::Degraded);
        assert_eq!(source_health(0, 4), SourceHealth::Down);
        assert_eq!(source_dot_color(SourceHealth::Healthy, &pal), pal.ink3);
        assert_eq!(source_dot_color(SourceHealth::Degraded, &pal), pal.warn);
        assert_eq!(source_dot_color(SourceHealth::Down, &pal), pal.crit);
        for h in [SourceHealth::Healthy, SourceHealth::Degraded, SourceHealth::Down] {
            assert_ne!(source_dot_color(h, &pal), pal.watch, "green is reserved for watched values");
        }
    }

    #[test]
    fn the_popover_goes_where_it_covers_the_least_of_the_other_cards() {
        let screen = egui::Rect::from_min_size(pos2(0.0, 0.0), vec2(2000.0, 1000.0));
        let size = vec2(380.0, 260.0);
        // two cards side by side; the row is in the left one
        let left = egui::Rect::from_min_size(pos2(10.0, 50.0), vec2(500.0, 700.0));
        let right = egui::Rect::from_min_size(pos2(522.0, 50.0), vec2(500.0, 700.0));
        let row = egui::Rect::from_min_size(pos2(10.0, 120.0), vec2(500.0, 28.0));
        let p = popover_pos(row, size, screen, &[right]);
        let r = egui::Rect::from_min_size(p, size);
        assert!(!r.intersects(right), "not over the neighbouring card: {r:?}");
        assert!(!r.intersects(row), "never over the row itself");
        assert!(left.contains_rect(r), "inside its own card instead: {r:?}");
        // nothing to avoid: beside the row, as before
        let p = popover_pos(row, size, screen, &[]);
        assert_eq!(p.x, row.right() + 8.0);
        // always on screen
        let low_row = egui::Rect::from_min_size(pos2(1700.0, 900.0), vec2(290.0, 28.0));
        let r = egui::Rect::from_min_size(popover_pos(low_row, size, screen, &[]), size);
        assert!(screen.contains_rect(r), "{r:?}");
        assert!(!r.intersects(low_row));
    }

    #[test]
    fn the_right_dock_split_respects_its_minimums() {
        let h = 12.0;
        // a roomy window: the saved share
        let w = dock_right_width(0.42, 2400.0, h);
        assert!((w - 1008.0).abs() < 0.01, "{w}");
        // a small share is lifted to the table's minimum
        assert_eq!(dock_right_width(0.1, 2400.0, h), MIN_PANEL_W);
        // a big share leaves the sensors their minimum and the divider
        let big = dock_right_width(0.9, 2400.0, h);
        assert!((big - (2400.0 - h - MIN_SENSORS_W)).abs() < 0.01, "{big}");
        // always in range, for any window and share
        for total in [900.0f32, 1100.0, 1500.0, 2400.0] {
            for f in [0.0f32, 0.2, 0.42, 0.7, 1.0] {
                let w = dock_right_width(f, total, h);
                assert!(w >= 0.0 && w <= total - h - MIN_SENSORS_W + 0.01, "{total} {f}: {w}");
                if total >= MIN_PANEL_W + MIN_SENSORS_W + h {
                    assert!(w >= MIN_PANEL_W - 0.01, "{total} {f}: {w}");
                }
            }
        }
        // too narrow for both: the sensors keep theirs, the panel gets what is left
        let w = dock_right_width(0.42, 900.0, h);
        assert!((w - (900.0 - h - MIN_SENSORS_W)).abs() < 0.01);
        assert_eq!(dock_right_width(0.5, 400.0, h), 0.0);
    }

    #[test]
    fn the_compact_right_panel_is_sized_in_pixels_within_its_limits() {
        let h = 12.0;
        let min = 420.0;
        assert_eq!(dock_right_width_px(500.0, min, 2400.0, h), 500.0);
        // a saved width below the minimum (340) renders at the minimum
        assert_eq!(dock_right_width_px(340.0, min, 2400.0, h), min);
        assert_eq!(dock_right_width_px(100.0, min, 2400.0, h), min);
        assert!((dock_right_width_px(5000.0, min, 2400.0, h) - (2400.0 - h - MIN_SENSORS_W)).abs() < 0.01);
        // the sensors keep their minimum in a small window
        assert!(dock_right_width_px(500.0, min, 900.0, h) <= 900.0 - h - MIN_SENSORS_W + 0.01);
    }

    #[test]
    fn the_bottom_panel_keeps_about_five_rows_in_a_small_window() {
        let h = 12.0;
        let rows5 = crate::procs::min_panel_height(crate::procs::MIN_PANEL_ROWS);
        let rows1 = crate::procs::min_panel_height(1.0);
        assert!(rows5 > rows1 + 3.0 * 26.0 - 1.0, "five rows are four rows more than one: {rows5} against {rows1}");
        // the 1200x700 window: about 580 px for sensors and panel. 30 % used to leave 170 px,
        // about 1.5 rows; now the panel keeps its five rows
        let total = 580.0;
        let got = panel_height(0.3 * total, total, h, rows5);
        assert!(got >= rows5 - 0.01, "{got} against {rows5}");
        assert!(got > 170.0 + 2.0 * 26.0, "{got} is clearly more than the old floor");
        // dragging it smaller stops at the floor too
        assert_eq!(panel_height(40.0, total, h, rows5), rows5);
        // a tall window is not forced: the share wins above the floor
        assert_eq!(panel_height(700.0, 2000.0, h, rows5), 700.0);
        // the sensors keep their 120 px even when five rows would not fit
        let tiny = panel_height(0.3 * 330.0, 330.0, h, rows5);
        assert!(tiny <= 330.0 - 120.0 - h + 0.01, "{tiny}");
    }

    #[test]
    fn the_top_bar_gives_up_since_then_source_words_then_legend_names_before_it_overlaps() {
        let w = BarWidths { left: [520.0, 440.0, 330.0], right_fixed: 300.0, sources: Some([140.0, 36.0]), since: Some(110.0) };
        let need_full = 520.0 + BAR_GAP + 300.0 + 140.0 + 110.0;
        assert_eq!(bar_density(need_full, &w), BarDensity::Full);
        assert_eq!(bar_density(need_full - 1.0, &w), BarDensity::NoSince);
        assert_eq!(bar_density(need_full - 110.0 - 1.0, &w), BarDensity::SourcesDot);
        assert_eq!(bar_density(520.0 + BAR_GAP + 300.0 + 36.0 - 1.0, &w), BarDensity::ShortLegend);
        assert_eq!(bar_density(440.0 + BAR_GAP + 300.0 + 36.0 - 1.0, &w), BarDensity::DotLegend);
        assert_eq!(bar_density(100.0, &w), BarDensity::DotLegend, "never worse than dots");
        // each step only takes things away
        assert!(BarDensity::Full.since() && !BarDensity::NoSince.since());
        assert_eq!(BarDensity::NoSince.sources(), SourcesForm::Text);
        assert_eq!(BarDensity::SourcesDot.sources(), SourcesForm::Dot);
        assert_eq!(BarDensity::ShortLegend.legend(), LegendForm::Short);
        assert_eq!(BarDensity::DotLegend.legend(), LegendForm::Dot);
        // without data there is no Sources chip and no Since text to count
        let bare = BarWidths { sources: None, since: None, ..w };
        assert_eq!(bar_density(520.0 + BAR_GAP + 300.0, &bare), BarDensity::Full);
    }

    fn numbers() -> StatusNumbers {
        StatusNumbers {
            procs: Some((666, 101)),
            machine: Some(StatusMachine { cpu: "13.0 %".into(), gpu: "39.0 %".into(), ram: Some((25.3 * 1024.0 * 1024.0 * 1024.0, 62.0 * 1024.0 * 1024.0 * 1024.0, 41.0)) }),
        }
    }

    #[test]
    fn the_status_bar_gives_up_the_groups_then_the_ram_total_then_the_process_count() {
        let text = |d| status_segments(d, &numbers()).iter().map(|s| s.text.clone()).collect::<Vec<_>>().join(" ");
        assert_eq!(text(StatusDensity::Full), "666 processes in 101 groups CPU 13.0 % GPU 39.0 % Memory 25.3 GiB / 62.0 GiB (41 %)");
        assert_eq!(text(StatusDensity::NoGroups), "666 processes CPU 13.0 % GPU 39.0 % Memory 25.3 GiB / 62.0 GiB (41 %)");
        assert_eq!(text(StatusDensity::ShortMemory), "666 processes CPU 13.0 % GPU 39.0 % Memory 25.3 GiB (41 %)");
        assert_eq!(text(StatusDensity::Bare), "CPU 13.0 % GPU 39.0 % Memory 41 %");
        // each form is narrower than the one before, so the first that fits is the fullest that fits
        let w = StatusWidths { left: [700.0, 560.0, 480.0, 300.0], right: 160.0 };
        assert_eq!(status_density(1000.0, &w), StatusDensity::Full);
        assert_eq!(status_density(800.0, &w), StatusDensity::NoGroups);
        assert_eq!(status_density(700.0, &w), StatusDensity::ShortMemory);
        assert_eq!(status_density(600.0, &w), StatusDensity::Bare);
        assert_eq!(status_density(100.0, &w), StatusDensity::Bare, "the most compact when none fits");
        // the right end's width counts: a toast takes room from the left
        let toast = StatusWidths { right: 400.0, ..w };
        assert!(status_density(1000.0, &toast) > StatusDensity::Full);
    }

    #[test]
    fn legend_names_shorten_and_a_dot_chip_is_narrower() {
        assert_eq!(LegendForm::Full.text(Device::Memory), "Memory");
        assert_eq!(LegendForm::Short.text(Device::Memory), "Mem");
        assert_eq!(LegendForm::Full.text(Device::Drive), "Drives");
        assert_eq!(LegendForm::Short.text(Device::Drive), "Drive");
        assert_eq!(LegendForm::Short.text(Device::Igpu), "iGPU");
        assert_eq!(LegendForm::Dot.text(Device::Cpu), "");
        let full = legend_chip_width(60.0, None, LegendForm::Full);
        assert!(legend_chip_width(60.0, None, LegendForm::Dot) < full - 50.0);
        assert!(legend_chip_width(60.0, Some(14.0), LegendForm::Full) > full, "a state mark takes room");
    }

    #[test]
    fn the_sources_chip_is_quiet_until_a_source_fails() {
        assert_eq!(sources_text(4, 4), "Sources");
        assert_eq!(sources_text(0, 0), "Sources");
        assert_eq!(sources_text(3, 4), "Sources 3 of 4");
        assert_eq!(sources_text(0, 4), "Sources 0 of 4");
    }

    #[test]
    fn reading_names_come_from_the_snapshot_and_never_show_an_id() {
        let snap = crate::demo::Demo::new().snapshot(1.0);
        let name = reading_name(&snap, "cpu/temp/tctl");
        assert!(name.starts_with("CPU, "), "{name}");
        assert!(!name.contains('/'), "{name}");
        assert_eq!(reading_name(&snap, "gone/sensor"), "That value");
    }

    #[test]
    fn the_kill_dialog_words_both_actions_the_way_the_buttons_do() {
        assert_eq!(Signal::Term.label(), "End task");
        assert_eq!(Signal::Kill.label(), "Force kill");
        assert!(kill_blurb(false).starts_with("End task "));
        assert!(kill_blurb(true).starts_with("Force kill "));
        for force in [false, true] {
            let t = kill_blurb(force);
            assert!(!t.contains("SIG"), "{t}");
        }
    }

    #[test]
    fn the_unreadable_settings_notice_names_the_saved_copy() {
        let bad = bad_copy_path(std::path::Path::new("/home/alex/.config/hwglance/ui.json"));
        assert_eq!(bad, PathBuf::from("/home/alex/.config/hwglance/ui.json.bad"));
        let before = CorruptNotice { bad: bad.clone(), saved: false };
        let after = CorruptNotice { bad, saved: true };
        let (b, a) = (before.text("/home/alex"), after.text("/home/alex"));
        assert!(b.contains("~/.config/hwglance/ui.json.bad") && b.contains("when the first change is saved"), "{b}");
        assert!(a.contains("~/.config/hwglance/ui.json.bad") && a.contains("is saved as"), "{a}");
        assert_ne!(a, b);
    }
}
