//! Whether the window can be seen right now, and the repaint calls that honour it.
//!
//! A window nobody can see must not paint. On Wayland winit reports no visibility, so eframe
//! paints a hidden window whenever something asks for a frame, and with vsync on a buffer swap
//! waits for a frame callback that Hyprland sends only to windows it draws. A window on a
//! workspace that no monitor shows, or under a fullscreen window, never gets one: the swap
//! blocks the event-loop thread, the window stops answering the compositor's pings, and Hyprland
//! offers to terminate it. Vsync stays on and the frame rate is not capped, so the only cure is
//! to ask for no frames while nothing is shown.
//!
//! Under Hyprland a thread follows the compositor's event socket and, on every event that can
//! move or cover a window, asks `j/clients` and `j/monitors` where the window is. The answer is
//! one flag. Every repaint request that does not come from egui itself goes through
//! [`repaint`] or [`repaint_after`], which drop the request while the flag says hidden. Data
//! keeps arriving and the next frame reads it; the collector is unaffected.
//!
//! The window counts as hidden when
//! - its workspace (normal or special) is not the one a monitor shows, or that monitor is off
//!   (disabled, or in DPMS sleep), or
//! - Hyprland does not draw it (minimized, or behind the front tab of a group), or
//! - another window on its workspace is maximized or fullscreen and this one is not. Hyprland
//!   draws only the fullscreen window of a workspace, in either mode (`hasfullscreen`), except
//!   floating windows that were opened over it or are pinned. `j/clients` does not say which
//!   floating window was, so a floating or pinned window is never counted as covered: a missed
//!   cover costs what the window did before this module, a false one would freeze a window that
//!   is on screen.
//!
//! Everything else counts as shown: no Hyprland, no socket, an answer that cannot be read, a
//! query that times out, a window the compositor does not list yet.

use crate::winctl::{self, Client};
use egui::Context;
use std::io::{BufRead, BufReader};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// How long the event thread waits for an event before it asks anyway. Events cover every move of
/// a window; this covers what has no event (a monitor going to sleep or waking) and an event that
/// was missed.
const BEAT: Duration = Duration::from_secs(2);
/// After an event, how long to wait for the rest of its burst (a workspace switch sends several)
/// before asking once.
const BURST: Duration = Duration::from_millis(5);
/// The longest a burst of events may delay the check.
const MAX_BURST: Duration = Duration::from_millis(50);
/// Pause before the event socket is opened again after it closed or could not be opened.
const RETRY: Duration = Duration::from_secs(1);

/// Events that can change what is shown or where a window is. The list is deliberately short:
/// focus and title changes arrive many times a second in a game.
const EVENTS: &[&str] = &[
    "workspace",
    "workspacev2",
    "focusedmon",
    "focusedmonv2",
    "createworkspace",
    "createworkspacev2",
    "destroyworkspace",
    "destroyworkspacev2",
    "moveworkspace",
    "moveworkspacev2",
    "renameworkspace",
    "activespecial",
    "activespecialv2",
    "fullscreen",
    "openwindow",
    "closewindow",
    "movewindow",
    "movewindowv2",
    "changefloatingmode",
    "pin",
    "monitoradded",
    "monitoraddedv2",
    "monitorremoved",
    "monitorremovedv2",
    "configreloaded",
    "dpms",
];

/// The flag, in its own type so the tests can have their own.
struct Shown(AtomicBool);

impl Shown {
    const fn new() -> Shown {
        Shown(AtomicBool::new(true))
    }

    fn get(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }

    /// Store the new state; returns whether it differs from the old one.
    fn set(&self, shown: bool) -> bool {
        self.0.swap(shown, Ordering::Relaxed) != shown
    }
}

static SHOWN: Shown = Shown::new();

/// Whether the window can be seen. True until the compositor says otherwise.
pub fn shown() -> bool {
    SHOWN.get()
}

/// `ctx.request_repaint()`, unless the window is hidden. For every request from outside egui's
/// own drawing.
pub fn repaint(ctx: &Context) {
    if SHOWN.get() {
        ctx.request_repaint();
    }
}

/// `ctx.request_repaint_after(after)`, unless the window is hidden.
pub fn repaint_after(ctx: &Context, after: Duration) {
    if SHOWN.get() {
        ctx.request_repaint_after(after);
    }
}

/// What is needed of a monitor of Hyprland's `j/monitors`.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Monitor {
    /// Id of the workspace it shows.
    workspace: i64,
    /// Id of the special workspace shown over it, 0 for none.
    special: i64,
    /// Switched on: not disabled and not asleep (DPMS).
    on: bool,
}

/// The monitors of `j/monitors`, or none when the answer cannot be read.
fn parse_monitors(json: &str) -> Vec<Monitor> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else { return Vec::new() };
    let id = |m: &serde_json::Value, key: &str| m.get(key).and_then(|w| w.get("id")).and_then(|x| x.as_i64());
    let flag = |m: &serde_json::Value, key: &str, default: bool| m.get(key).and_then(|x| x.as_bool()).unwrap_or(default);
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|m| {
                    Some(Monitor {
                        workspace: id(m, "activeWorkspace")?,
                        special: id(m, "specialWorkspace").unwrap_or(0),
                        on: !flag(m, "disabled", false) && flag(m, "dpmsStatus", true),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Whether the window of process `pid` can be seen, from the two answers.
fn decide(clients: &[Client], monitors: &[Monitor], pid: u32) -> bool {
    let Some(me) = winctl::own_client(clients, pid) else { return true };
    let Some(workspace) = me.workspace else { return true };
    let on: Vec<&Monitor> = monitors.iter().filter(|m| m.on).collect();
    if on.is_empty() {
        // no monitor to judge by (or none readable)
        return true;
    }
    if me.hidden || !on.iter().any(|m| m.workspace == workspace || (m.special != 0 && m.special == workspace)) {
        return false;
    }
    let free = me.fullscreen != 0 || me.floating || me.pinned;
    let covered = clients.iter().any(|c| c.address != me.address && c.workspace == me.workspace && c.fullscreen != 0 && !c.hidden);
    free || !covered
}

/// Whether an event line (`name>>data`) is one that can change the answer.
fn relevant(line: &[u8]) -> bool {
    let name = line.split(|&b| b == b'>').next().unwrap_or_default();
    EVENTS.iter().any(|e| e.as_bytes() == name)
}

/// Ask Hyprland where the window is. Errors are the caller's to treat as shown.
fn probe() -> Result<bool, String> {
    let clients = winctl::parse_clients(&winctl::hypr_query("j/clients")?);
    let monitors = parse_monitors(&winctl::hypr_query("j/monitors")?);
    Ok(decide(&clients, &monitors, std::process::id()))
}

/// Store the answer; on the way back to shown, ask for a frame at once so the numbers are fresh.
fn publish(ctx: &Context, shown: bool) {
    if SHOWN.set(shown) {
        hwglance_proto::crash::note(&format!("window {} (no frames are drawn while it is hidden)", if shown { "shown" } else { "hidden" }));
        if shown {
            ctx.request_repaint();
        }
    }
}

/// Follow Hyprland's events for the life of the process. Does nothing outside Hyprland.
pub fn spawn(ctx: Context) {
    if !winctl::under_hyprland() {
        return;
    }
    let _ = std::thread::Builder::new().name("visibility".into()).spawn(move || loop {
        follow(&ctx);
        // the socket is gone (Hyprland restarted or stopped): say shown until it is back
        publish(&ctx, true);
        std::thread::sleep(RETRY);
    });
}

/// One connection to the event socket; returns when it closes or cannot be used.
fn follow(ctx: &Context) {
    let Some(path) = winctl::hypr_event_socket() else { return };
    let Ok(stream) = UnixStream::connect(path) else { return };
    // events from before this point are not delivered, so ask once now
    publish(ctx, probe().unwrap_or(true));
    let mut reader = BufReader::new(stream);
    let mut line = Vec::new();
    let mut asked = Instant::now();
    // when the first event not yet answered came in
    let mut woken: Option<Instant> = None;
    loop {
        let wait = if woken.is_some() { BURST } else { BEAT };
        if reader.get_ref().set_read_timeout(Some(wait)).is_err() {
            return;
        }
        let quiet = match reader.read_until(b'\n', &mut line) {
            Ok(0) => return,
            Ok(_) => {
                if line.ends_with(b"\n") {
                    if relevant(&line) {
                        woken.get_or_insert_with(Instant::now);
                    }
                    line.clear();
                }
                false
            }
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => true,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => false,
            Err(_) => return,
        };
        // Ask when the burst is over, or a beat has passed (events that never stop must not
        // starve the check).
        if quiet || woken.is_some_and(|t| t.elapsed() >= MAX_BURST) || asked.elapsed() >= BEAT {
            woken = None;
            asked = Instant::now();
            publish(ctx, probe().unwrap_or(true));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Windows of a made-up session: a terminal on workspace 2 and a game on 3, plus `extra`.
    /// Ours is pid 4242.
    fn clients(extra: &[&str]) -> Vec<Client> {
        let base = [
            r#"{"address":"0xa","pid":10,"class":"foot","fullscreen":0,"workspace":{"id":2,"name":"2"},"floating":false,"pinned":false,"hidden":false}"#,
            r#"{"address":"0xb","pid":11,"class":"steam_app_1","fullscreen":0,"workspace":{"id":3,"name":"3"},"floating":false,"pinned":false,"hidden":false}"#,
        ];
        winctl::parse_clients(&format!("[{}]", base.iter().chain(extra).copied().collect::<Vec<_>>().join(",")))
    }

    fn ours(workspace: i64) -> String {
        format!(r#"{{"address":"0xc","pid":4242,"class":"hwglance","workspace":{{"id":{workspace},"name":"{workspace}"}},"fullscreen":0,"floating":false,"pinned":false,"hidden":false}}"#)
    }

    fn cover(mode: u8, workspace: i64) -> String {
        format!(r#"{{"address":"0xd","pid":77,"class":"game","fullscreen":{mode},"workspace":{{"id":{workspace},"name":"{workspace}"}},"floating":false,"pinned":false,"hidden":false}}"#)
    }

    /// Workspaces 1 and 2 belong to monitor DP-1 (2 is shown), 3 to DP-2 (shown), 4 to neither.
    fn monitors() -> Vec<Monitor> {
        parse_monitors(
            r#"[
              {"name":"DP-1","activeWorkspace":{"id":2,"name":"2"},"specialWorkspace":{"id":0,"name":""},"dpmsStatus":true,"disabled":false},
              {"name":"DP-2","activeWorkspace":{"id":3,"name":"3"},"specialWorkspace":{"id":0,"name":""},"dpmsStatus":true,"disabled":false}
            ]"#,
        )
    }

    fn shown_with(own: &str, others: &[&str]) -> bool {
        let extra: Vec<&str> = std::iter::once(own).chain(others.iter().copied()).collect();
        decide(&clients(&extra), &monitors(), 4242)
    }

    #[test]
    fn a_window_on_a_workspace_a_monitor_shows_is_shown() {
        assert!(shown_with(&ours(2), &[]));
        assert!(shown_with(&ours(3), &[]), "the workspace of the second monitor counts as well");
    }

    #[test]
    fn a_window_on_a_workspace_no_monitor_shows_is_hidden() {
        assert!(!shown_with(&ours(1), &[]));
        assert!(!shown_with(&ours(4), &[]));
    }

    #[test]
    fn a_special_workspace_is_shown_only_while_a_monitor_has_it_open() {
        let all = clients(&[&ours(-98)]);
        assert!(!decide(&all, &monitors(), 4242));
        let open = parse_monitors(
            r#"[{"activeWorkspace":{"id":2,"name":"2"},"specialWorkspace":{"id":-98,"name":"special:magic"},"dpmsStatus":true,"disabled":false}]"#,
        );
        assert!(decide(&all, &open, 4242));
        // the normal workspace under it is still drawn
        assert!(decide(&clients(&[&ours(2)]), &open, 4242));
    }

    #[test]
    fn a_monitor_that_is_off_shows_nothing() {
        let all = clients(&[&ours(2)]);
        let two = |dpms: bool, disabled: bool| {
            parse_monitors(&format!(
                r#"[{{"activeWorkspace":{{"id":2,"name":"2"}},"specialWorkspace":{{"id":0,"name":""}},"dpmsStatus":{dpms},"disabled":{disabled}}},
                    {{"activeWorkspace":{{"id":3,"name":"3"}},"specialWorkspace":{{"id":0,"name":""}},"dpmsStatus":true,"disabled":false}}]"#
            ))
        };
        assert!(decide(&all, &two(true, false), 4242));
        assert!(!decide(&all, &two(false, false), 4242), "asleep");
        assert!(!decide(&all, &two(true, true), 4242), "disabled");
        // every monitor off: nothing to judge by
        let none = parse_monitors(r#"[{"activeWorkspace":{"id":2,"name":"2"},"dpmsStatus":false}]"#);
        assert!(decide(&all, &none, 4242));
    }

    #[test]
    fn a_fullscreen_or_maximized_window_of_the_workspace_covers_ours() {
        for mode in [1, 2, 3] {
            assert!(!shown_with(&ours(2), &[&cover(mode, 2)]), "mode {mode}");
        }
        assert!(shown_with(&ours(2), &[&cover(0, 2)]));
    }

    #[test]
    fn a_cover_on_another_workspace_or_one_that_is_not_drawn_does_not_count() {
        assert!(shown_with(&ours(2), &[&cover(2, 3)]));
        assert!(shown_with(&ours(2), &[&cover(2, 2).replace(r#""hidden":false"#, r#""hidden":true"#)]));
    }

    #[test]
    fn a_fullscreen_window_of_ours_is_shown_and_so_is_one_that_floats_or_is_pinned() {
        for mode in [1, 2, 3] {
            let own = ours(2).replace(r#""fullscreen":0"#, &format!(r#""fullscreen":{mode}"#));
            assert!(shown_with(&own, &[&cover(2, 2)]), "mode {mode}");
        }
        assert!(shown_with(&ours(2).replace(r#""floating":false"#, r#""floating":true"#), &[&cover(2, 2)]));
        assert!(shown_with(&ours(2).replace(r#""pinned":false"#, r#""pinned":true"#), &[&cover(2, 2)]));
    }

    #[test]
    fn a_window_the_compositor_does_not_draw_is_hidden_even_on_a_shown_workspace() {
        assert!(!shown_with(&ours(2).replace(r#""hidden":false"#, r#""hidden":true"#), &[]));
    }

    #[test]
    fn a_window_that_moves_between_monitors_follows_its_new_workspace() {
        // 3 is on the second monitor, 1 is a workspace of the first one that is not shown
        assert!(shown_with(&ours(3), &[]));
        assert!(!shown_with(&ours(1), &[]));
        assert!(shown_with(&ours(2), &[]));
    }

    #[test]
    fn what_cannot_be_judged_counts_as_shown() {
        let m = monitors();
        // unreadable answers
        assert!(decide(&winctl::parse_clients("not json"), &m, 4242));
        assert!(decide(&clients(&[&ours(1)]), &parse_monitors("not json"), 4242));
        assert!(decide(&clients(&[&ours(1)]), &parse_monitors("[]"), 4242));
        // not (yet) among the clients, or listed without a workspace
        assert!(decide(&clients(&[]), &m, 4242));
        let bare = r#"{"address":"0xc","pid":4242,"class":"hwglance","fullscreen":0}"#;
        assert!(decide(&clients(&[bare]), &m, 4242));
        // another process's window is not ours
        assert!(decide(&clients(&[&ours(1)]), &m, 99));
    }

    #[test]
    fn only_events_that_move_or_cover_a_window_wake_the_check() {
        for e in ["workspacev2>>2,2\n", "activespecialv2>>-98,special:magic,DP-1\n", "fullscreen>>1\n", "movewindowv2>>0xc,2,2\n", "monitorremoved>>DP-2\n", "dpms>>DP-1,0\n"] {
            assert!(relevant(e.as_bytes()), "{e}");
        }
        for e in ["activewindow>>foot,~\n", "activewindowv2>>0xc\n", "windowtitle>>0xc\n", "activelayout>>kbd,us\n", "workspacefoo>>1\n", "\n"] {
            assert!(!relevant(e.as_bytes()), "{e}");
        }
    }

    #[test]
    fn the_flag_reports_only_a_real_change() {
        let s = Shown::new();
        assert!(s.get());
        assert!(!s.set(true));
        assert!(s.set(false));
        assert!(!s.get());
        assert!(!s.set(false));
        assert!(s.set(true));
    }
}
