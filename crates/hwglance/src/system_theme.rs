//! The desktop's light or dark setting, from the XDG settings portal.
//!
//! winit 0.30 does not report the system theme on Wayland, so the portal is asked directly:
//! `org.freedesktop.portal.Settings.ReadOne("org.freedesktop.appearance", "color-scheme")` once at
//! start, then one long-running `gdbus monitor` child for its `SettingChanged` signal. Omarchy's
//! theme switch sets the GNOME `color-scheme` and the portal relays it, so the window follows a
//! theme change live. A child process costs no binary size and, blocked on a pipe, no CPU; a
//! D-Bus crate would add both. Without `gdbus` or the portal the answer is dark.

use crate::theme::Scheme;
use crate::visibility;
use std::io::{BufRead, BufReader};
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

const DEST: &str = "org.freedesktop.portal.Desktop";
const PATH: &str = "/org/freedesktop/portal/desktop";

/// What the app reads each frame: whether the desktop currently asks for light.
pub type SystemLight = Arc<AtomicBool>;

pub fn scheme_of(light: &SystemLight) -> Scheme {
    if light.load(Ordering::Relaxed) { Scheme::Light } else { Scheme::Dark }
}

/// The portal's `color-scheme` value: 1 prefers dark, 2 prefers light, 0 has no preference. Dark
/// is the answer for 0 and for anything else.
pub fn scheme_from_portal(value: u32) -> Scheme {
    if value == 2 { Scheme::Light } else { Scheme::Dark }
}

/// The number in the first `uint32 N` of gdbus' printout: `(<uint32 1>,)`, `<uint32 2>)`.
fn uint32_in(text: &str) -> Option<u32> {
    let rest = &text[text.find("uint32 ")? + "uint32 ".len()..];
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// A `ReadOne` reply as `gdbus call` prints it. A failure or garbage is dark.
pub fn parse_reply(text: &str) -> Scheme {
    uint32_in(text).map(scheme_from_portal).unwrap_or_default()
}

/// A line of `gdbus monitor`: only the `SettingChanged` of `color-scheme` counts.
/// `/org/freedesktop/portal/desktop: org.freedesktop.portal.Settings.SettingChanged
/// ('org.freedesktop.appearance', 'color-scheme', <uint32 2>)`
pub fn parse_signal(line: &str) -> Option<Scheme> {
    if !line.contains("SettingChanged") || !line.contains("org.freedesktop.appearance") || !line.contains("'color-scheme'") {
        return None;
    }
    Some(uint32_in(line).map(scheme_from_portal).unwrap_or_default())
}

/// Ask the portal now. Blocks for the few milliseconds the call takes; a portal that does not
/// answer within two seconds counts as dark, so the first frame is never held up for long.
pub fn read_now() -> Scheme {
    let out = Command::new("gdbus")
        .args(["call", "--session", "--timeout", "2", "--dest", DEST, "--object-path", PATH])
        .args(["--method", "org.freedesktop.portal.Settings.ReadOne", "org.freedesktop.appearance", "color-scheme"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output();
    match out {
        Ok(o) if o.status.success() => parse_reply(&String::from_utf8_lossy(&o.stdout)),
        _ => Scheme::Dark,
    }
}

/// Follow the portal in a background thread: `flag` is kept current and the window repaints on
/// every change. The monitor child dies with the app (parent-death signal).
pub fn spawn(ctx: egui::Context, flag: SystemLight) {
    let publish = move |s: Scheme| {
        let light = s == Scheme::Light;
        if flag.swap(light, Ordering::Relaxed) != light {
            visibility::repaint(&ctx);
        }
    };
    let _ = std::thread::Builder::new().name("system-theme".into()).spawn(move || {
        loop {
            let mut cmd = Command::new("gdbus");
            cmd.args(["monitor", "--session", "--dest", DEST, "--object-path", PATH])
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null());
            // SAFETY: only async-signal-safe prctl between fork and exec. The signal is tied to
            // this thread, which lives as long as the child's output does.
            unsafe {
                cmd.pre_exec(|| {
                    libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM as libc::c_ulong);
                    Ok(())
                });
            }
            let Ok(mut child) = cmd.spawn() else { return };
            // Read after the monitor is up, so a change in between is not lost.
            publish(read_now());
            if let Some(out) = child.stdout.take() {
                for line in BufReader::new(out).lines().map_while(Result::ok) {
                    if let Some(s) = parse_signal(&line) {
                        publish(s);
                    }
                }
            }
            let _ = child.wait();
            // The bus went away; try again without spinning.
            std::thread::sleep(Duration::from_secs(30));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portal_values() {
        assert_eq!(scheme_from_portal(1), Scheme::Dark);
        assert_eq!(scheme_from_portal(2), Scheme::Light);
        assert_eq!(scheme_from_portal(0), Scheme::Dark);
        assert_eq!(scheme_from_portal(7), Scheme::Dark);
    }

    #[test]
    fn replies_parse_and_garbage_is_dark() {
        assert_eq!(parse_reply("(<uint32 1>,)\n"), Scheme::Dark);
        assert_eq!(parse_reply("(<uint32 2>,)\n"), Scheme::Light);
        assert_eq!(parse_reply("(<<uint32 2>>,)"), Scheme::Light);
        assert_eq!(parse_reply("(<uint32 0>,)"), Scheme::Dark);
        assert_eq!(parse_reply("(<uint32 12>,)"), Scheme::Dark);
        assert_eq!(parse_reply(""), Scheme::Dark);
        assert_eq!(parse_reply("Error: GDBus.Error:org.freedesktop.portal.Error.NotFound"), Scheme::Dark);
        assert_eq!(parse_reply("(<uint32 >,)"), Scheme::Dark);
        assert_eq!(parse_reply("uint32 two"), Scheme::Dark);
    }

    #[test]
    fn only_the_colour_scheme_signal_counts() {
        let line = |ns: &str, key: &str, v: u32| {
            format!("/org/freedesktop/portal/desktop: org.freedesktop.portal.Settings.SettingChanged ('{ns}', '{key}', <uint32 {v}>)")
        };
        assert_eq!(parse_signal(&line("org.freedesktop.appearance", "color-scheme", 2)), Some(Scheme::Light));
        assert_eq!(parse_signal(&line("org.freedesktop.appearance", "color-scheme", 1)), Some(Scheme::Dark));
        assert_eq!(parse_signal(&line("org.freedesktop.appearance", "color-scheme", 0)), Some(Scheme::Dark));
        assert_eq!(parse_signal(&line("org.freedesktop.appearance", "accent-color", 2)), None);
        assert_eq!(parse_signal(&line("org.gnome.desktop.interface", "color-scheme", 2)), None);
        assert_eq!(parse_signal("Monitoring signals on object /org/freedesktop/portal/desktop owned by org.freedesktop.portal.Desktop"), None);
        assert_eq!(parse_signal("garbage"), None);
        assert_eq!(parse_signal(""), None);
    }

    #[test]
    fn the_watcher_publishes_what_the_portal_says() {
        // Whatever this machine's portal answers (or dark without one), the flag ends up there.
        let flag: SystemLight = Arc::new(AtomicBool::new(false));
        spawn(egui::Context::default(), flag.clone());
        let want = read_now();
        let start = std::time::Instant::now();
        while scheme_of(&flag) != want && start.elapsed() < Duration::from_secs(3) {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(scheme_of(&flag), want);
    }

    #[test]
    fn the_flag_maps_to_a_scheme() {
        let f: SystemLight = Arc::new(AtomicBool::new(false));
        assert_eq!(scheme_of(&f), Scheme::Dark);
        f.store(true, Ordering::Relaxed);
        assert_eq!(scheme_of(&f), Scheme::Light);
    }
}
