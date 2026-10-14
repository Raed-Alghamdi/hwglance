//! The window controls at the right end of the top bar: maximize or restore, and close. The
//! desktop draws no title bar, so the window carries its own.
//!
//! Close and maximize are viewport commands, which Hyprland honours (it maps a maximize request
//! to its "maximized" mode). What it does not do is report that mode back: a tiled window there
//! always says it is maximized, so the first click would ask to restore a window that is not
//! maximized. Under Hyprland the state therefore comes from its `j/clients` query, asked on its
//! own socket (no process is started) off the UI thread under a timeout, so a stuck compositor
//! never freezes a frame.

use crate::a11y::{self, Ring, Spoken};
use crate::theme::{self, base, Palette};
use crate::ui_kit;
use egui::{vec2, Align2, Context, Sense, Ui};
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Gap between the gear and the window controls, and between the controls themselves.
const GROUP_GAP: f32 = 12.0;
const BUTTON_GAP: f32 = 2.0;
/// How long one socket read or write may take before the query is given up on.
const HYPR_TIMEOUT: Duration = Duration::from_secs(2);
/// How often a focused window under Hyprland asks whether it is still maximized (a bind can
/// change that without the window being told).
const POLL_EVERY: Duration = Duration::from_secs(3);
/// How long the compositor gets to act on a maximize request before its state is read.
const SETTLE: Duration = Duration::from_millis(400);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control {
    Maximize,
    Close,
}

/// The window's size state: what egui reports, or under Hyprland what Hyprland says.
pub struct Controls {
    hypr: bool,
    /// Under Hyprland: whether the compositor has the window in its maximized mode.
    maximized: Arc<AtomicBool>,
    busy: Arc<AtomicBool>,
    polled: Option<Instant>,
}

impl Controls {
    /// `compositor` turns on the Hyprland path (it still needs a running Hyprland); the tests
    /// leave it off so they never reach the desktop they run on.
    pub fn new(compositor: bool) -> Controls {
        Controls { hypr: compositor && under_hyprland(), maximized: Arc::default(), busy: Arc::default(), polled: None }
    }

    pub fn is_maximized(&self, ctx: &Context) -> bool {
        if self.hypr { self.maximized.load(Ordering::Relaxed) } else { viewport_maximized(ctx) }
    }

    /// Call once a frame: under Hyprland, read the maximized state at the start and then now and
    /// then while the window has the focus.
    pub fn tick(&mut self, ctx: &Context, focused: bool) {
        let due = self.polled.is_none_or(|t| t.elapsed() >= POLL_EVERY && focused);
        if self.hypr && due {
            self.polled = Some(Instant::now());
            self.refresh(ctx, Duration::ZERO);
        }
    }

    pub fn toggle_maximize(&self, ctx: &Context) {
        let want = !self.is_maximized(ctx);
        ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(want));
        if self.hypr {
            // show the change at once, then check what the compositor made of the request
            self.maximized.store(want, Ordering::Relaxed);
            self.refresh(ctx, SETTLE);
        }
    }

    /// Off the UI thread, after `wait`: read whether Hyprland has the window maximized.
    fn refresh(&self, ctx: &Context, wait: Duration) {
        if self.busy.swap(true, Ordering::AcqRel) {
            return;
        }
        let (ctx, maximized, busy) = (ctx.clone(), self.maximized.clone(), self.busy.clone());
        std::thread::spawn(move || {
            std::thread::sleep(wait);
            if let Ok(now) = own_window().map(|c| c.maximized()) {
                maximized.store(now, Ordering::Relaxed);
            }
            busy.store(false, Ordering::Release);
            ctx.request_repaint();
        });
    }
}

fn viewport_maximized(ctx: &Context) -> bool {
    ctx.input(|i| i.viewport().maximized).unwrap_or(false)
}

/// A Hyprland session (it sets this variable for every client).
pub fn under_hyprland() -> bool {
    std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some_and(|v| !v.is_empty())
}

/// Same as closing the window any other way: the collector keeps running.
pub fn close(ctx: &Context) {
    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
}

/// How the window came to close, for the journal line written on exit.
#[derive(Debug, Default, Clone, Copy)]
pub struct CloseTrace {
    /// The window's own Close button sent the close command.
    pub button: bool,
    /// A close request reached the window (from the button, or from outside).
    pub requested: bool,
}

impl CloseTrace {
    /// Note this frame's close request. Call every frame.
    pub fn observe(&mut self, ctx: &Context) {
        self.requested |= ctx.input(|i| i.viewport().close_requested());
    }

    /// The close reason, as one line for the journal.
    pub fn reason(&self) -> &'static str {
        match (self.button, self.requested) {
            (true, _) => "closed by its own Close button",
            (false, true) => "closed by a close request from the compositor or window manager (not the Close button)",
            (false, false) => "event loop ended without a close request",
        }
    }
}

/// The name a control speaks and shows in its tooltip.
pub fn control_name(control: Control, maximized: bool) -> &'static str {
    match control {
        Control::Maximize if maximized => "Restore",
        Control::Maximize => "Maximize",
        Control::Close => "Close",
    }
}

fn control_glyph(control: Control, maximized: bool) -> char {
    match control {
        Control::Maximize if maximized => ui_kit::icon::WINDOW_RESTORE,
        Control::Maximize => ui_kit::icon::WINDOW_MAXIMIZE,
        Control::Close => ui_kit::icon::WINDOW_CLOSE,
    }
}

fn control_hint(control: Control, maximized: bool) -> &'static str {
    match control {
        Control::Maximize if maximized => "Returns the window to its earlier size. Double-clicking an empty part of the top bar does the same.",
        Control::Maximize => "Makes the window fill the screen. Double-clicking an empty part of the top bar does the same.",
        Control::Close => "Closes the window. The collector keeps running.",
    }
}

/// Width of the group of two beside a gear `button_w` wide: the gap, the buttons and what is
/// between them. The top bar reserves it before it decides how much to show.
pub fn group_width(button_w: f32) -> f32 {
    GROUP_GAP + 2.0 * button_w.max(theme::MIN_HIT) + BUTTON_GAP
}

/// One ghost button of the group: no frame at rest, the hover wash of the other buttons, and for
/// Close the critical colour with its own text colour on it.
fn button(ui: &mut Ui, pal: &Palette, control: Control, maximized: bool, size: egui::Vec2) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let danger = control == Control::Close;
    let (wash, ink) = if danger && (resp.hovered() || resp.is_pointer_button_down_on()) {
        (Some(pal.crit), pal.on_crit)
    } else if resp.is_pointer_button_down_on() {
        (Some(pal.selected), pal.ink)
    } else if resp.hovered() {
        (Some(pal.hover), pal.ink)
    } else {
        (None, pal.ink2)
    };
    if let Some(wash) = wash {
        ui.painter().rect_filled(rect, 6.0, wash);
    }
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, control_glyph(control, maximized), ui_kit::f_icon(base() + 2.0), ink);
    a11y::focus_ring(ui, &resp, 6.0, Ring::Outside, pal);
    let name = control_name(control, maximized);
    a11y::publish(&resp, || Spoken::button(name).described(control_hint(control, maximized)));
    resp.on_hover_text(name)
}

/// Maximize or restore, then close, in the size of the gear before them. Returns the one that
/// was clicked.
pub fn show_group(ui: &mut Ui, pal: &Palette, maximized: bool, size: egui::Vec2) -> Option<Control> {
    ui.add_space(GROUP_GAP - ui.spacing().item_spacing.x);
    let mut clicked = None;
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.x = BUTTON_GAP;
        let size = vec2(size.x.max(theme::MIN_HIT), size.y.max(theme::MIN_HIT));
        for control in [Control::Maximize, Control::Close] {
            if button(ui, pal, control, maximized, size).clicked() {
                clicked = Some(control);
            }
        }
    });
    clicked
}

// ---------------------------------------------------------------------------
// Hyprland
// ---------------------------------------------------------------------------

/// What is needed of a window of Hyprland's `j/clients`.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Client {
    address: String,
    pid: u32,
    class: String,
    /// `fullscreen` of `j/clients`: 0 none, 1 maximized, 2 fullscreen, 3 both.
    fullscreen: u8,
}

impl Client {
    fn maximized(&self) -> bool {
        self.fullscreen & 1 == 1
    }
}

/// The windows of `j/clients`; entries it cannot read are skipped.
fn parse_clients(json: &str) -> Vec<Client> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else { return Vec::new() };
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|c| {
                    Some(Client {
                        address: c.get("address")?.as_str()?.to_string(),
                        pid: u32::try_from(c.get("pid")?.as_u64()?).ok()?,
                        class: c.get("class").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
                        fullscreen: u8::try_from(c.get("fullscreen")?.as_u64()?).ok()?,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// This process's window: the one of its pid, the main window ("hwglance" class) first.
fn own_client(clients: &[Client], pid: u32) -> Option<&Client> {
    let mine = || clients.iter().filter(|c| c.pid == pid);
    mine().find(|c| c.class == "hwglance").or_else(|| mine().next())
}

/// Hyprland's command socket for this session.
fn hypr_socket() -> Option<PathBuf> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")?;
    let sig = std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").filter(|v| !v.is_empty())?;
    Some(PathBuf::from(runtime).join("hypr").join(sig).join(".socket.sock"))
}

/// Ask Hyprland one query on its socket (`hyprctl` does the same) and return its answer.
fn hypr_query(request: &str) -> Result<String, String> {
    let sock = hypr_socket().ok_or("no Hyprland socket")?;
    let mut s = UnixStream::connect(&sock).map_err(|e| format!("Hyprland socket: {e}"))?;
    s.set_read_timeout(Some(HYPR_TIMEOUT)).ok();
    s.set_write_timeout(Some(HYPR_TIMEOUT)).ok();
    s.write_all(request.as_bytes()).map_err(|e| format!("Hyprland socket: {e}"))?;
    let mut out = String::new();
    s.read_to_string(&mut out).map_err(|e| format!("Hyprland socket: {e}"))?;
    Ok(out)
}

fn own_window() -> Result<Client, String> {
    let all = parse_clients(&hypr_query("j/clients")?);
    own_client(&all, std::process::id()).cloned().ok_or_else(|| "this window is not among Hyprland's clients".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_trace_tells_the_button_from_the_compositor() {
        let reason = |button, requested| CloseTrace { button, requested }.reason();
        assert!(reason(true, true).contains("Close button"));
        assert!(reason(false, true).contains("compositor or window manager"));
        assert!(reason(false, false).contains("without a close request"));
        assert_ne!(reason(true, true), reason(false, true));
    }

    #[test]
    fn maximize_says_restore_while_maximized_and_each_control_has_its_own_glyph() {
        assert_eq!(control_name(Control::Maximize, false), "Maximize");
        assert_eq!(control_name(Control::Maximize, true), "Restore");
        assert_eq!(control_name(Control::Close, false), "Close");
        let glyphs: std::collections::HashSet<char> =
            [(Control::Maximize, false), (Control::Maximize, true), (Control::Close, false)].iter().map(|(c, m)| control_glyph(*c, *m)).collect();
        assert_eq!(glyphs.len(), 3);
        assert!(glyphs.iter().all(|g| (*g as u32) >= 0xF0001), "Material glyphs of the Nerd Font");
    }

    #[test]
    fn the_close_hover_keeps_its_text_readable_in_both_palettes() {
        let lum = |c: egui::Color32| {
            let f = |v: u8| {
                let v = v as f64 / 255.0;
                if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
            };
            0.2126 * f(c.r()) + 0.7152 * f(c.g()) + 0.0722 * f(c.b())
        };
        for (name, p) in [("dark", Palette::dark()), ("light", Palette::light())] {
            let (a, b) = (lum(p.on_crit), lum(p.crit));
            let ratio = (a.max(b) + 0.05) / (a.min(b) + 0.05);
            assert!(ratio >= 4.5, "{name}: {ratio:.2}");
        }
    }

    const CLIENTS: &str = r#"[
      {"address":"0x6409b720c880","pid":10,"class":"com.edde746.plezy","fullscreen":0},
      {"address":"0x6409b7aaaaaa","pid":4242,"class":"hwglance","fullscreen":1},
      {"address":"0x6409b7bbbbbb","pid":4242,"class":"other","fullscreen":0},
      {"nope":1}
    ]"#;

    #[test]
    fn the_own_window_is_found_by_pid_and_the_unreadable_entries_are_skipped() {
        let all = parse_clients(CLIENTS);
        assert_eq!(all.len(), 3);
        let own = own_client(&all, 4242).unwrap();
        assert_eq!(own.address, "0x6409b7aaaaaa");
        assert!(own.maximized());
        assert!(own_client(&all, 99).is_none());
        assert!(parse_clients("not json").is_empty());
        // without the app's class the window of the pid still counts; fullscreen alone is not maximized
        let other = parse_clients(r#"[{"address":"0x1f","pid":5,"class":"x","fullscreen":2}]"#);
        assert!(!own_client(&other, 5).unwrap().maximized());
        assert!(Client { fullscreen: 3, ..other[0].clone() }.maximized());
    }

    #[test]
    fn without_hyprland_the_state_is_the_viewports_and_nothing_else_is_touched() {
        let c = Controls::new(false);
        assert!(!c.hypr);
        let ctx = Context::default();
        assert!(!c.is_maximized(&ctx));
        c.toggle_maximize(&ctx);
        assert!(!c.busy.load(Ordering::Acquire), "no helper thread was started");
    }

    #[test]
    fn the_group_is_two_hit_areas_and_a_gap_wide() {
        assert_eq!(group_width(40.0), GROUP_GAP + 80.0 + BUTTON_GAP);
        assert!(group_width(10.0) >= GROUP_GAP + 2.0 * theme::MIN_HIT);
    }
}
