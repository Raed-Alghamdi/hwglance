//! Connection to the collector daemon (background thread), plus the shared snapshot slot
//! that the demo and mock sources publish into as well.

use hwglance_proto::{self as proto, ClientMsg, DaemonMsg, HistorySeries, Snapshot};
use std::collections::HashMap;
use std::io::BufReader;
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Default)]
pub struct Shared {
    pub snapshot: Option<Arc<Snapshot>>,
    pub received: Option<Instant>,
    /// Shown instead of the real age of the snapshot in "updated .. s ago" (tests with golden images).
    pub pinned_age: Option<f32>,
    /// Bumped for every accepted snapshot; the UI compares it to know when to rebuild.
    pub counter: u64,
    pub connected: bool,
    /// At least one connection attempt has failed (so the banner does not flash at startup).
    pub connect_failed: bool,
    pub last_error: Option<String>,
    /// Never mark data stale (mock mode).
    pub never_stale: bool,
    /// Latest history reply per reading id, with the time it arrived.
    pub history: HashMap<String, (Instant, Arc<HistorySeries>)>,
    /// When a history request for an id was last sent (to pace the 2 s refresh).
    history_asked: HashMap<String, Instant>,
    /// A `Focus` message waiting for the window to act on it.
    pub focus: Option<String>,
    writer: Option<UnixStream>,
    /// The process panel is hidden, so the collector need not send processes (also on the next connect).
    procs_off: bool,
    /// The window has no focus, so the collector may sample processes less often (also on the
    /// next connect).
    unfocused: bool,
    /// Why the last "start" or "restart" of the collector service failed; cleared by the next
    /// attempt and once the collector answers.
    pub action_error: Option<String>,
    /// Bumped for every failed attempt, so the window can announce each one once.
    pub action_failures: u64,
    /// The collector keeps sending messages this window cannot read (another version, most
    /// likely); cleared by the next snapshot that can be.
    pub unreadable: bool,
    /// Stats resets and undos sent, in order, for the demo generator to apply.
    demo_stats: Vec<ClientMsg>,
    demo_history: Vec<(Vec<String>, u32)>,
    /// Limit changes sent (`SetThreshold`, `ResetThreshold`), for the demo generator to apply; a
    /// test reads them to see what the window asked for.
    demo_limits: Vec<ClientMsg>,
}

#[derive(Clone)]
pub struct Link {
    pub shared: Arc<Mutex<Shared>>,
}

impl Link {
    pub fn new() -> Self {
        Link { shared: Arc::new(Mutex::new(Shared::default())) }
    }

    /// The shared state. A thread that panicked while holding the lock must not take the window
    /// down with it: the data is plain values, so the guard is taken from the poisoned lock.
    pub fn lock(&self) -> std::sync::MutexGuard<'_, Shared> {
        self.shared.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Store a new snapshot and wake the UI.
    pub fn publish(&self, ctx: &egui::Context, snap: Snapshot) {
        {
            let mut s = self.lock();
            s.snapshot = Some(Arc::new(snap));
            s.received = Some(Instant::now());
            s.counter += 1;
            s.connected = true;
            s.unreadable = false;
        }
        ctx.request_repaint();
    }

    /// Store a history reply and wake the UI.
    pub fn publish_history(&self, ctx: &egui::Context, series: Vec<HistorySeries>) {
        {
            let mut s = self.lock();
            for h in series {
                s.history.insert(h.id.clone(), (Instant::now(), Arc::new(h)));
            }
        }
        ctx.request_repaint();
    }

    /// The newest history of a reading and how old it is.
    pub fn history_of(&self, id: &str) -> Option<(Arc<HistorySeries>, Duration)> {
        let s = self.lock();
        s.history.get(id).map(|(at, h)| (h.clone(), at.elapsed()))
    }

    /// Ask for the history of one reading unless it was asked for less than `every` ago.
    /// Returns true when a request was sent.
    pub fn request_history(&self, id: &str, seconds: u32, every: Duration) -> bool {
        {
            let mut s = self.lock();
            if s.history_asked.get(id).is_some_and(|t| t.elapsed() < every) {
                return false;
            }
            s.history_asked.insert(id.to_string(), Instant::now());
        }
        self.send(&ClientMsg::History { ids: vec![id.to_string()], seconds })
    }

    pub fn take_focus(&self) -> Option<String> {
        self.lock().focus.take()
    }

    pub fn take_demo_limits(&self) -> Vec<ClientMsg> {
        std::mem::take(&mut self.lock().demo_limits)
    }

    pub fn take_demo_history(&self) -> Vec<(Vec<String>, u32)> {
        std::mem::take(&mut self.lock().demo_history)
    }

    /// Send a command to the daemon (or to the demo generator). Returns false when not connected.
    pub fn send(&self, msg: &ClientMsg) -> bool {
        let mut s = self.lock();
        match msg {
            ClientMsg::SetProcs { enabled, unfocused } => {
                s.procs_off = !*enabled;
                s.unfocused = *unfocused;
            }
            ClientMsg::ResetStats | ClientMsg::UndoResetStats => {
                if s.demo_stats.len() >= 16 {
                    s.demo_stats.remove(0);
                }
                s.demo_stats.push(msg.clone());
            }
            ClientMsg::History { ids, seconds } => s.demo_history.push((ids.clone(), *seconds)),
            ClientMsg::SetThreshold { .. } | ClientMsg::ResetThreshold { .. } => {
                if s.demo_limits.len() >= 256 {
                    s.demo_limits.remove(0);
                }
                s.demo_limits.push(msg.clone());
            }
            _ => {}
        }
        match s.writer.as_mut() {
            Some(w) => proto::write_msg(w, msg).is_ok(),
            None => false,
        }
    }

    /// Say whether the process feed is wanted: the next (re)connect subscribes accordingly.
    /// Changing it on a live connection goes through `send(SetProcs)`, which also lands here.
    pub fn set_procs_wanted(&self, wanted: bool) {
        self.lock().procs_off = !wanted;
    }

    /// Say whether the window has focus: the next (re)connect subscribes accordingly. A live
    /// connection is told through `send(SetProcs)`.
    pub fn set_unfocused(&self, unfocused: bool) {
        self.lock().unfocused = unfocused;
    }

    pub fn take_demo_stats(&self) -> Vec<ClientMsg> {
        std::mem::take(&mut self.lock().demo_stats)
    }
}

impl Default for Link {
    fn default() -> Self {
        Self::new()
    }
}

/// Connect, subscribe and read snapshots forever; retry every second while disconnected.
pub fn spawn_socket(ctx: egui::Context, link: Link) {
    std::thread::Builder::new()
        .name("collector-link".into())
        .spawn(move || loop {
            match UnixStream::connect(proto::socket_path()) {
                Ok(stream) => {
                    // a panic in the session (a message nobody expected) ends this connection, not
                    // the thread, so the link reconnects on its own
                    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| session(&ctx, &link, stream))).is_err() {
                        let mut s = link.lock();
                        s.writer = None;
                        s.connected = false;
                        s.connect_failed = true;
                        s.last_error = Some("The connection to the collector broke unexpectedly. Reconnecting.".to_string());
                        drop(s);
                        ctx.request_repaint();
                    }
                }
                Err(e) => {
                    let reason = connect_reason(&e);
                    let mut s = link.lock();
                    if !s.connect_failed || s.connected || s.last_error.as_deref() != Some(reason.as_str()) {
                        s.connect_failed = true;
                        s.connected = false;
                        s.last_error = Some(reason);
                        drop(s);
                        ctx.request_repaint();
                    }
                }
            }
            std::thread::sleep(Duration::from_secs(1));
        })
        .expect("spawn collector-link thread");
}

/// Why the socket could not be reached, in words for the "Collector not running" message.
pub fn connect_reason(e: &std::io::Error) -> String {
    use std::io::ErrorKind;
    match e.kind() {
        ErrorKind::NotFound => "The collector's socket does not exist, so the service is not running.".to_string(),
        ErrorKind::ConnectionRefused => "The socket exists but nothing answers on it: the service stopped or crashed.".to_string(),
        ErrorKind::PermissionDenied => "The collector's socket exists but this user may not open it.".to_string(),
        _ => format!("Could not reach the collector's socket: {e}."),
    }
}

/// How many messages in a row may fail to parse before the window says it cannot read the collector.
const UNREADABLE_RUN: u32 = 3;

fn session(ctx: &egui::Context, link: &Link, stream: UnixStream) {
    let _ = stream.set_write_timeout(Some(Duration::from_millis(300)));
    let Ok(mut writer) = stream.try_clone() else { return };
    // Under the lock, so a SetProcs sent meanwhile is either seen here or written after the writer is stored.
    {
        let mut s = link.lock();
        let (procs, unfocused) = (!s.procs_off, s.unfocused);
        if proto::write_msg(&mut writer, &ClientMsg::Subscribe { procs, slim: true, unfocused }).is_err() {
            return;
        }
        s.writer = Some(writer);
        s.connected = true;
        s.connect_failed = false;
        s.last_error = None;
        s.action_error = None;
        s.unreadable = false;
    }
    ctx.request_repaint();

    let mut reader = BufReader::new(stream);
    let mut buf = String::new();
    let mut known = StaticCache::default();
    let mut unreadable = 0u32;
    loop {
        match proto::read_msg::<_, DaemonMsg>(&mut reader, &mut buf) {
            Ok(Some(DaemonMsg::Snapshot(mut snap))) => {
                known.fill(&mut snap);
                unreadable = 0;
                link.publish(ctx, *snap)
            }
            Ok(Some(DaemonMsg::History { series })) => link.publish_history(ctx, series),
            Ok(Some(DaemonMsg::Focus { id })) => {
                link.lock().focus = Some(id);
                ctx.request_repaint();
            }
            Ok(Some(DaemonMsg::Error { message })) => {
                link.lock().last_error = Some(message);
                ctx.request_repaint();
            }
            Ok(None) => break,
            Err(e) if e.kind() == std::io::ErrorKind::InvalidData => {
                // one garbled line is skipped; a run of them is a collector of another version
                unreadable += 1;
                if unreadable == UNREADABLE_RUN {
                    link.lock().unreadable = true;
                    ctx.request_repaint();
                }
            }
            Err(_) => break,
        }
    }
    {
        let mut s = link.lock();
        s.writer = None;
        s.connected = false;
        s.connect_failed = true;
    }
    ctx.request_repaint();
}

/// What the collector sends once per process (slim subscription): the command line and the other
/// fields that never change.
#[derive(Clone)]
struct Known {
    cmdline: String,
    exe: Option<String>,
    user: String,
    started_unix_ms: u64,
}

/// The static fields of every process seen so far, by pid and start time, to fill into the
/// records that arrive without them.
#[derive(Default)]
struct StaticCache(HashMap<(i32, u64), Known>);

impl StaticCache {
    /// Complete the process records of a snapshot and forget processes that are gone. A record
    /// the collector has not described yet (we joined between two full rounds) stays empty
    /// until the next round.
    fn fill(&mut self, snap: &mut Snapshot) {
        let Some(groups) = snap.procs.as_mut() else { return };
        let mut old = std::mem::take(&mut self.0);
        for p in groups.iter_mut().flat_map(|g| g.procs.iter_mut()) {
            let key = (p.pid, p.start_time_ticks);
            if p.has_static() {
                old.remove(&key);
                self.0.insert(
                    key,
                    Known { cmdline: p.cmdline.clone(), exe: p.exe.clone(), user: p.user.clone(), started_unix_ms: p.started_unix_ms },
                );
            } else if let Some(k) = old.remove(&key) {
                p.cmdline.clone_from(&k.cmdline);
                p.exe.clone_from(&k.exe);
                p.user.clone_from(&k.user);
                p.started_unix_ms = k.started_unix_ms;
                self.0.insert(key, k);
            }
        }
    }
}

/// Parse a mock file: a bare `Snapshot`, a `DaemonMsg` document, or a log of `DaemonMsg` lines
/// (the last snapshot wins).
pub fn parse_mock(text: &str) -> Result<Snapshot, String> {
    if let Ok(DaemonMsg::Snapshot(s)) = serde_json::from_str::<DaemonMsg>(text) {
        return Ok(*s);
    }
    if let Ok(s) = serde_json::from_str::<Snapshot>(text) {
        return Ok(s);
    }
    let mut last = None;
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        if let Ok(DaemonMsg::Snapshot(s)) = serde_json::from_str::<DaemonMsg>(line) {
            last = Some(*s);
        } else if let Ok(s) = serde_json::from_str::<Snapshot>(line) {
            last = Some(s);
        }
    }
    last.ok_or_else(|| "no snapshot found in file".to_string())
}

/// What to tell the user about the collector's protocol version, if it differs from the one this
/// window speaks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VersionNote {
    /// The collector is older than the window (restarting it may be enough).
    pub older: bool,
    pub text: String,
}

/// `Some` when the collector's protocol version is not this window's.
pub fn version_note(collector: u32) -> Option<VersionNote> {
    let window = proto::PROTO_VERSION;
    if collector == window {
        return None;
    }
    let older = collector < window;
    let text = if older {
        format!(
            "The collector is older than this window (it speaks protocol {collector}, the window needs {window}), so some readings may be missing. Restart it with systemctl --user restart hwglanced, or reinstall HW Glance."
        )
    } else {
        format!(
            "The collector is newer than this window (it speaks protocol {collector}, the window speaks {window}). Reinstall HW Glance and restart this window."
        )
    };
    Some(VersionNote { older, text })
}

/// Run a program and say, in words, why it failed: its own message when it printed one.
fn run_reporting(program: &str, args: &[&str]) -> Result<(), String> {
    let out = std::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .output()
        .map_err(|e| format!("Could not run {program}: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    let said = String::from_utf8_lossy(&out.stderr);
    let said = said.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    let said: String = said.chars().take(240).collect();
    Err(if said.is_empty() { format!("{program} exited with {}", out.status) } else { said })
}

/// Run `systemctl --user <verb> hwglanced` off the UI thread; a failure is kept in the link, for
/// the collector-down screen and a toast.
fn run_unit_action(link: &Link, ctx: &egui::Context, verb: &'static str) {
    link.lock().action_error = None;
    let (link, ctx) = (link.clone(), ctx.clone());
    std::thread::spawn(move || {
        if let Err(e) = run_reporting("systemctl", &["--user", verb, "hwglanced"]) {
            let mut s = link.lock();
            s.action_error = Some(format!("Could not {verb} the collector: {e}"));
            s.action_failures += 1;
        }
        ctx.request_repaint();
    });
}

/// Start the collector with systemd, never blocking the UI.
pub fn start_collector(link: &Link, ctx: &egui::Context) {
    run_unit_action(link, ctx, "start");
}

/// Restart the collector (to apply a new sample interval), never blocking the UI.
pub fn restart_collector(link: &Link, ctx: &egui::Context) {
    run_unit_action(link, ctx, "restart");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap_json() -> String {
        let s = crate::demo::Demo::new().snapshot(0.0);
        serde_json::to_string(&s).unwrap()
    }

    #[test]
    fn history_and_focus_messages_reach_the_link() {
        use std::io::Write;
        let (a, mut b) = UnixStream::pair().unwrap();
        let link = Link::new();
        let l2 = link.clone();
        let ctx = egui::Context::default();
        let t = std::thread::spawn(move || session(&ctx, &l2, a));
        let hist = DaemonMsg::History {
            series: vec![HistorySeries { id: "cpu/temp/tctl".into(), unit: proto::Unit::Celsius, points: vec![(1000, 50.0), (2000, 52.5)] }],
        };
        proto::write_msg(&mut b, &hist).unwrap();
        proto::write_msg(&mut b, &DaemonMsg::Focus { id: "gpu0/temp/hotspot".into() }).unwrap();
        b.write_all(b"this is not json\n").unwrap();
        let snap = crate::demo::Demo::new().snapshot(1.0);
        proto::write_msg(&mut b, &DaemonMsg::Snapshot(Box::new(snap))).unwrap();
        drop(b);
        t.join().unwrap();
        let (series, _) = link.history_of("cpu/temp/tctl").expect("history stored");
        assert_eq!(series.points.len(), 2);
        assert_eq!(link.take_focus().as_deref(), Some("gpu0/temp/hotspot"));
        assert_eq!(link.take_focus(), None, "taken once");
        assert!(link.lock().snapshot.is_some(), "a bad line does not stop the stream");
    }

    #[test]
    fn a_run_of_unreadable_messages_is_reported_and_one_good_snapshot_clears_it() {
        use std::io::Write;
        let (a, mut b) = UnixStream::pair().unwrap();
        let link = Link::new();
        let l2 = link.clone();
        let ctx = egui::Context::default();
        let t = std::thread::spawn(move || session(&ctx, &l2, a));
        // a collector of another version: valid JSON, but not a message this window knows
        b.write_all(b"{\"Snapshot\":{\"proto\":99}}\n{\"Mystery\":1}\n[1,2]\n").unwrap();
        // wait until the session has read them (it flags the run)
        for _ in 0..200 {
            if link.lock().unreadable {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(link.lock().unreadable, "three unreadable messages in a row are reported");
        assert!(link.lock().snapshot.is_none());
        proto::write_msg(&mut b, &DaemonMsg::Snapshot(Box::new(crate::demo::Demo::new().snapshot(1.0)))).unwrap();
        drop(b);
        t.join().unwrap();
        assert!(!link.lock().unreadable, "a snapshot that parses clears it");
        assert!(link.lock().snapshot.is_some());
    }

    #[test]
    fn one_garbled_line_is_not_reported() {
        use std::io::Write;
        let (a, mut b) = UnixStream::pair().unwrap();
        let link = Link::new();
        let l2 = link.clone();
        let ctx = egui::Context::default();
        let t = std::thread::spawn(move || session(&ctx, &l2, a));
        b.write_all(b"not json\nnor this\n").unwrap();
        drop(b);
        t.join().unwrap();
        assert!(!link.lock().unreadable);
    }

    #[test]
    fn a_thread_that_panicked_under_the_lock_does_not_break_the_window() {
        let link = Link::new();
        let l2 = link.clone();
        let _ = std::thread::spawn(move || {
            let _held = l2.shared.lock().unwrap();
            panic!("the link thread died holding the lock");
        })
        .join();
        assert!(link.shared.is_poisoned());
        // every reader goes through lock(), which takes the guard from the poisoned lock
        assert!(link.lock().snapshot.is_none());
        link.publish(&egui::Context::default(), crate::demo::Demo::new().snapshot(1.0));
        assert!(link.lock().snapshot.is_some());
        assert!(link.take_focus().is_none());
    }

    fn proc(pid: i32, with_static: bool) -> proto::ProcInfo {
        let mut p = crate::demo::Demo::new().snapshot(0.0).procs.unwrap().remove(0).procs.remove(0);
        p.pid = pid;
        p.start_time_ticks = pid as u64 * 10;
        if with_static {
            p.cmdline = format!("/usr/bin/p{pid} --x");
            p.exe = Some(format!("/usr/bin/p{pid}"));
            p.user = "alex".into();
            p.started_unix_ms = 5_000 + pid as u64;
        } else {
            p.cmdline.clear();
            p.exe = None;
            p.user.clear();
            p.started_unix_ms = 0;
        }
        p
    }

    fn snap_with(procs: Vec<proto::ProcInfo>) -> Snapshot {
        let mut s = crate::demo::Demo::new().snapshot(0.0);
        let mut g = s.procs.take().unwrap().remove(0);
        g.procs = procs;
        s.procs = Some(vec![g]);
        s
    }

    #[test]
    fn slim_records_are_completed_from_earlier_full_ones() {
        let mut cache = StaticCache::default();
        let mut first = snap_with(vec![proc(1, true), proc(2, true)]);
        cache.fill(&mut first);
        let mut next = snap_with(vec![proc(1, false), proc(2, false), proc(3, false), proc(4, true)]);
        cache.fill(&mut next);
        let procs = &next.procs.as_ref().unwrap()[0].procs;
        assert_eq!(procs[0].cmdline, "/usr/bin/p1 --x");
        assert_eq!((procs[0].user.as_str(), procs[0].started_unix_ms), ("alex", 5_001));
        assert_eq!(procs[1].exe.as_deref(), Some("/usr/bin/p2"));
        assert!(!procs[2].has_static(), "never described: stays empty until the next full round");
        assert!(procs[3].has_static());
        // process 2 left, 4 stays known, a different start time is a different process
        let mut third = snap_with(vec![proc(1, false), proc(4, false)]);
        cache.fill(&mut third);
        assert!(third.procs.as_ref().unwrap()[0].procs.iter().all(|p| p.has_static()));
        let mut reused = proc(1, false);
        reused.start_time_ticks += 1;
        let mut fourth = snap_with(vec![reused, proc(2, false)]);
        cache.fill(&mut fourth);
        assert!(fourth.procs.as_ref().unwrap()[0].procs.iter().all(|p| !p.has_static()), "pid 1 was reused, pid 2 was forgotten");
        // a snapshot without processes leaves the cache alone
        let mut lite = crate::demo::Demo::new().snapshot(0.0);
        lite.procs = None;
        cache.fill(&mut lite);
        assert!(lite.procs.is_none());
    }

    #[test]
    fn subscription_follows_the_process_panel() {
        for (wanted, expect) in [(true, true), (false, false)] {
            let (a, b) = UnixStream::pair().unwrap();
            let link = Link::new();
            link.set_procs_wanted(wanted);
            let l2 = link.clone();
            let ctx = egui::Context::default();
            let t = std::thread::spawn(move || session(&ctx, &l2, a));
            let mut line = String::new();
            std::io::BufRead::read_line(&mut BufReader::new(b.try_clone().unwrap()), &mut line).unwrap();
            let msg: ClientMsg = serde_json::from_str(&line).unwrap();
            assert_eq!(msg, ClientMsg::Subscribe { procs: expect, slim: true, unfocused: false });
            // showing the panel while connected tells the collector and is remembered for reconnects
            assert!(link.send(&ClientMsg::SetProcs { enabled: !wanted, unfocused: false }));
            line.clear();
            std::io::BufRead::read_line(&mut BufReader::new(b.try_clone().unwrap()), &mut line).unwrap();
            assert_eq!(serde_json::from_str::<ClientMsg>(&line).unwrap(), ClientMsg::SetProcs { enabled: !wanted, unfocused: false });
            assert_eq!(link.lock().procs_off, wanted);
            drop(b);
            t.join().unwrap();
        }
    }

    #[test]
    fn the_window_focus_is_sent_and_remembered_for_reconnects() {
        // told while connected
        let (a, b) = UnixStream::pair().unwrap();
        let link = Link::new();
        let l2 = link.clone();
        let ctx = egui::Context::default();
        let t = std::thread::spawn(move || session(&ctx, &l2, a));
        let mut reader = BufReader::new(b.try_clone().unwrap());
        let mut line = String::new();
        std::io::BufRead::read_line(&mut reader, &mut line).unwrap();
        assert_eq!(serde_json::from_str::<ClientMsg>(&line).unwrap(), ClientMsg::Subscribe { procs: true, slim: true, unfocused: false });
        assert!(link.send(&ClientMsg::SetProcs { enabled: true, unfocused: true }));
        line.clear();
        std::io::BufRead::read_line(&mut reader, &mut line).unwrap();
        assert_eq!(line.trim(), r#"{"cmd":"set_procs","enabled":true,"unfocused":true}"#);
        drop(b);
        drop(reader);
        t.join().unwrap();
        // and the next connection starts out unfocused
        let (a, b) = UnixStream::pair().unwrap();
        let l2 = link.clone();
        let ctx = egui::Context::default();
        let t = std::thread::spawn(move || session(&ctx, &l2, a));
        line.clear();
        std::io::BufRead::read_line(&mut BufReader::new(b.try_clone().unwrap()), &mut line).unwrap();
        assert_eq!(serde_json::from_str::<ClientMsg>(&line).unwrap(), ClientMsg::Subscribe { procs: true, slim: true, unfocused: true });
        drop(b);
        t.join().unwrap();
    }

    #[test]
    fn a_protocol_mismatch_is_told_in_words() {
        assert_eq!(version_note(proto::PROTO_VERSION), None);
        let old = version_note(proto::PROTO_VERSION - 1).unwrap();
        assert!(old.older);
        assert!(old.text.contains("older") && old.text.contains("systemctl --user restart hwglanced") && old.text.contains("reinstall"), "{}", old.text);
        let new = version_note(proto::PROTO_VERSION + 1).unwrap();
        assert!(!new.older);
        assert!(new.text.contains("newer") && new.text.contains("Reinstall"), "{}", new.text);
        assert!(!old.text.contains('\u{2014}') && !new.text.contains('\u{2014}'));
    }

    #[test]
    fn a_failed_start_keeps_the_reason_and_a_good_one_clears_it() {
        let err = run_reporting("sh", &["-c", "echo 'Unit hwglanced.service not found.' >&2; echo more >&2; exit 5"]).unwrap_err();
        assert_eq!(err, "Unit hwglanced.service not found.");
        let err = run_reporting("sh", &["-c", "exit 7"]).unwrap_err();
        assert!(err.contains("exited with") && err.contains('7'), "{err}");
        let err = run_reporting("/nonexistent/systemctl", &[]).unwrap_err();
        assert!(err.starts_with("Could not run /nonexistent/systemctl"), "{err}");
        assert!(run_reporting("true", &[]).is_ok());
        // the link carries the failure, announces it once and clears it when the collector answers
        let link = Link::new();
        {
            let mut s = link.lock();
            s.action_error = Some("Could not start the collector: boom".into());
            s.action_failures = 1;
        }
        let (a, b) = UnixStream::pair().unwrap();
        let l2 = link.clone();
        let ctx = egui::Context::default();
        let t = std::thread::spawn(move || session(&ctx, &l2, a));
        std::io::BufRead::read_line(&mut BufReader::new(b.try_clone().unwrap()), &mut String::new()).unwrap();
        assert_eq!(link.lock().action_error, None);
        assert_eq!(link.lock().action_failures, 1);
        drop(b);
        t.join().unwrap();
    }

    #[test]
    fn history_requests_are_paced() {
        let link = Link::new();
        // not connected: nothing is written, but the request still counts as asked
        assert!(!link.request_history("a", 600, Duration::from_secs(2)));
        assert!(!link.request_history("a", 600, Duration::from_secs(2)));
        assert_eq!(link.take_demo_history().len(), 1, "the second call within 2 s was skipped");
        assert!(!link.request_history("b", 600, Duration::from_secs(2)));
        assert_eq!(link.take_demo_history(), vec![(vec!["b".to_string()], 600)]);
    }

    #[test]
    fn mock_accepts_bare_snapshot_and_daemon_line() {
        let bare = snap_json();
        let s = parse_mock(&bare).unwrap();
        assert!(!s.sensors.is_empty());
        let line = format!("{{\"type\":\"snapshot\",{}", &bare[1..]);
        let s2 = parse_mock(&line).unwrap();
        assert_eq!(s.hostname, s2.hostname);
        let two = format!("{line}\n{line}\n");
        assert!(parse_mock(&two).is_ok());
        assert!(parse_mock("{}").is_err());
    }
}

#[cfg(test)]
mod reason_tests {
    use super::connect_reason;
    use std::io::{Error, ErrorKind};

    #[test]
    fn a_missing_socket_and_a_refusing_one_read_differently() {
        let missing = connect_reason(&Error::from(ErrorKind::NotFound));
        let refused = connect_reason(&Error::from(ErrorKind::ConnectionRefused));
        assert!(missing.contains("does not exist") && refused.contains("nothing answers"));
        assert!(connect_reason(&Error::other("boom")).contains("boom"));
    }
}
