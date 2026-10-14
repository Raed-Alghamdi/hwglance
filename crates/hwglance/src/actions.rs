//! Ending processes: `libc::kill` for our own pids, `pkexec kill` for other users' pids.

use crate::model::{Target, TargetProc};
use crate::visibility;
use std::sync::mpsc::Sender;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Signal {
    Term,
    Kill,
}

impl Signal {
    /// The action as the window words it, on its button and in its messages.
    pub fn label(self) -> &'static str {
        match self {
            Signal::Term => "End task",
            Signal::Kill => "Force kill",
        }
    }

    fn libc(self) -> i32 {
        match self {
            Signal::Term => libc::SIGTERM,
            Signal::Kill => libc::SIGKILL,
        }
    }

    fn kill_arg(self) -> &'static str {
        match self {
            Signal::Term => "TERM",
            Signal::Kill => "KILL",
        }
    }
}

/// Show a folder in the file manager (`xdg-open`); it is not waited for.
pub fn open_folder(dir: &str) -> std::io::Result<()> {
    open_path(dir)
}

/// Open a file or folder with the desktop's default application (`xdg-open`); it is not waited for.
pub fn open_path(path: &str) -> std::io::Result<()> {
    std::process::Command::new("xdg-open")
        .arg(path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|mut child| {
            // reap it when it exits, so no zombie is left behind
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        })
}

/// Field 22 (starttime) of a `/proc/<pid>/stat` line. The name field may contain spaces and
/// parentheses, so parse from the last `)`.
pub fn parse_start_ticks(stat: &str) -> Option<u64> {
    let rest = &stat[stat.rfind(')')? + 1..];
    // fields after the name start at field 3 (state); starttime is field 22, index 19 here
    rest.split_whitespace().nth(19)?.parse().ok()
}

/// What `/proc` says about a target process right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Seen {
    /// Running (or not readable, which counts as running).
    Alive,
    /// No such process, or the pid belongs to another process now.
    Gone,
    /// Ended, but its parent has not collected it yet: there is nothing left to signal.
    Zombie,
}

fn seen(p: &TargetProc) -> Seen {
    // a pid of 0 or below would signal a whole process group or every process; never send those
    if p.pid <= 0 {
        return Seen::Gone;
    }
    match std::fs::read_to_string(format!("/proc/{}/stat", p.pid)) {
        Ok(s) => {
            if !parse_start_ticks(&s).is_none_or(|t| t == p.start_time_ticks) {
                Seen::Gone
            } else if matches!(stat_state(&s), "Z" | "X") {
                Seen::Zombie
            } else {
                Seen::Alive
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Seen::Gone,
        Err(_) => Seen::Alive,
    }
}

/// The state letter of a `/proc/<pid>/stat` line (the first field after the name).
fn stat_state(stat: &str) -> &str {
    stat.rfind(')').and_then(|i| stat[i + 1..].split_whitespace().next()).unwrap_or("")
}

/// Is this process still running? Its pid must still carry the same start time (a recycled pid
/// is another process), and a zombie has already ended. Not being able to read the entry
/// (another user's, or a hidepid mount) counts as running, so a "ended" is never wrongly claimed.
pub fn is_running(p: &TargetProc) -> bool {
    match std::fs::read_to_string(format!("/proc/{}/stat", p.pid)) {
        Ok(s) => !matches!(stat_state(&s), "Z" | "X") && parse_start_ticks(&s).is_none_or(|t| t == p.start_time_ticks),
        Err(e) => e.kind() != std::io::ErrorKind::NotFound,
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "process" } else { "processes" }
}

pub struct Outcome {
    /// Message to show right now.
    pub message: String,
    pub ok: bool,
    /// Other users' processes wait for polkit; their result arrives later on the channel.
    pub waiting_auth: bool,
}

/// Send `sig` to every process of `target`. Own pids are signalled directly; other users'
/// pids go through `pkexec /usr/bin/kill`, which runs on a thread so the UI never blocks
/// (polkit shows its own password prompt). The pkexec result arrives later on `tx`.
pub fn execute(target: &Target, sig: Signal, tx: Sender<(String, bool)>, ctx: egui::Context) -> Outcome {
    let me = unsafe { libc::getuid() };
    let mut sent = 0usize;
    let mut gone = 0usize;
    let mut zombies = 0usize;
    let mut denied = 0usize;
    let mut other: Vec<i32> = Vec::new();
    for p in &target.procs {
        match seen(p) {
            Seen::Gone => {
                gone += 1;
                continue;
            }
            Seen::Zombie => {
                zombies += 1;
                continue;
            }
            Seen::Alive => {}
        }
        if p.uid == me {
            let r = unsafe { libc::kill(p.pid, sig.libc()) };
            if r == 0 {
                sent += 1;
            } else if std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
                gone += 1;
            } else {
                denied += 1;
            }
        } else {
            other.push(p.pid);
        }
    }

    let waiting_auth = !other.is_empty();
    let mut parts = Vec::new();
    if sent > 0 {
        parts.push(format!("Sent {} to {sent} {}", sig.label(), plural(sent)));
    }
    if gone > 0 {
        parts.push(format!("{gone} already ended"));
    }
    if zombies > 0 {
        // a zombie takes no signal: it has ended and waits for its parent to collect it
        parts.push(format!("{zombies} already ended, waiting for its parent to clean up"));
    }
    if denied > 0 {
        parts.push(format!("{denied} not permitted"));
    }

    if !other.is_empty() {
        let n = other.len();
        parts.push(format!("waiting for authentication for {n} system {}", plural(n)));
        std::thread::spawn(move || {
            let mut cmd = std::process::Command::new("pkexec");
            cmd.args(["/usr/bin/kill", "-s", sig.kill_arg(), "--"]);
            for pid in &other {
                cmd.arg(pid.to_string());
            }
            let result = cmd.stdin(std::process::Stdio::null()).output();
            let (msg, ok) = match result {
                Ok(out) => pkexec_outcome(out.status.success(), out.status.code(), &String::from_utf8_lossy(&out.stderr), n, sig),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    ("pkexec is not installed, so other users' processes cannot be ended from here".to_string(), false)
                }
                Err(e) => (format!("Could not run pkexec: {e}"), false),
            };
            let _ = tx.send((msg, ok));
            visibility::repaint(&ctx);
        });
    }

    if parts.is_empty() {
        parts.push("Nothing to do".into());
    }
    Outcome { message: parts.join(", "), ok: denied == 0, waiting_auth }
}

/// What a finished `pkexec kill` means, in words. pkexec answers 126 when the prompt was
/// dismissed and 127 when authorisation failed or no agent could ask; anything else is kill's
/// own exit, whose complaint is the first line it printed.
fn pkexec_outcome(success: bool, code: Option<i32>, stderr: &str, n: usize, sig: Signal) -> (String, bool) {
    if success {
        return (format!("Sent {} to {n} system {}", sig.label(), plural(n)), true);
    }
    let lines: Vec<&str> = stderr.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    match code {
        Some(126) => ("Authentication dismissed, nothing was ended".to_string(), false),
        Some(127) if lines.iter().any(|l| l.contains("No authentication agent")) => {
            ("No authentication agent is running, so nothing was ended".to_string(), false)
        }
        Some(127) => ("Authentication failed, nothing was ended".to_string(), false),
        // every complaint is "no such process": they ended while the prompt was open
        _ if !lines.is_empty() && lines.iter().all(|l| l.contains("No such process")) => {
            ("Already ended before the request was approved".to_string(), true)
        }
        _ => {
            let first: String = lines.first().copied().unwrap_or("it gave no reason").chars().take(160).collect();
            (format!("pkexec kill failed: {first}"), false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_ticks_from_stat() {
        let line = "4242 (my (weird) name) S 1 4242 4242 0 -1 4194560 100 0 0 0 5 3 0 0 20 0 1 0 987654 1000 100 18446744073709551615";
        assert_eq!(parse_start_ticks(line), Some(987654));
        assert_eq!(parse_start_ticks("garbage"), None);
    }

    #[test]
    fn a_process_is_running_until_it_exits_and_a_recycled_pid_is_not_it() {
        let mut child = std::process::Command::new("sleep").arg("600").spawn().unwrap();
        let pid = child.id() as i32;
        let ticks = parse_start_ticks(&std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap()).unwrap();
        let p = TargetProc { pid, uid: 0, start_time_ticks: ticks, name: "sleep".into(), cmdline: String::new(), critical: None };
        assert!(is_running(&p));
        let other = TargetProc { start_time_ticks: ticks + 1, ..p.clone() };
        assert!(!is_running(&other), "same pid, other start time: another process");
        child.kill().unwrap();
        // killed but not yet waited for: a zombie has already ended
        for _ in 0..50 {
            if !is_running(&p) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(!is_running(&p));
        child.wait().unwrap();
        assert!(!is_running(&p), "gone");
    }

    fn dummy(pid: i32, uid: u32, ticks: u64) -> TargetProc {
        TargetProc { pid, uid, start_time_ticks: ticks, name: "x".into(), cmdline: String::new(), critical: None }
    }

    #[test]
    fn a_pid_that_would_signal_a_group_or_everything_is_never_sent() {
        let me = unsafe { libc::getuid() };
        for pid in [0, -1, -4242] {
            let t = Target { name: "bad".into(), procs: vec![dummy(pid, me, 1)] };
            let (tx, _rx) = std::sync::mpsc::channel();
            let out = execute(&t, Signal::Kill, tx, egui::Context::default());
            assert_eq!(out.message, "1 already ended", "pid {pid}");
            assert!(!out.waiting_auth);
        }
    }

    #[test]
    fn a_zombie_is_reported_as_ended_and_gets_no_signal() {
        // a child that exited and has not been waited for is a zombie
        let child = std::process::Command::new("true").spawn().unwrap();
        let pid = child.id() as i32;
        let mut ticks = None;
        for _ in 0..100 {
            if let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat"))
                && stat_state(&stat) == "Z"
            {
                ticks = parse_start_ticks(&stat);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let ticks = ticks.expect("the child became a zombie");
        let me = unsafe { libc::getuid() };
        let p = dummy(pid, me, ticks);
        assert_eq!(seen(&p), Seen::Zombie);
        assert!(!is_running(&p));
        let t = Target { name: "zombie".into(), procs: vec![p] };
        let (tx, _rx) = std::sync::mpsc::channel();
        let out = execute(&t, Signal::Term, tx, egui::Context::default());
        assert_eq!(out.message, "1 already ended, waiting for its parent to clean up");
        assert!(out.ok);
        let mut child = child;
        child.wait().unwrap();
    }

    #[test]
    fn pkexec_answers_are_told_in_words() {
        let t = |ok, code, err: &str| pkexec_outcome(ok, code, err, 2, Signal::Term);
        assert_eq!(t(true, Some(0), ""), ("Sent End task to 2 system processes".to_string(), true));
        assert_eq!(t(false, Some(126), "").0, "Authentication dismissed, nothing was ended");
        assert_eq!(t(false, Some(127), "Error getting authority: ...").0, "Authentication failed, nothing was ended");
        assert!(t(false, Some(127), "Error: No authentication agent found.").0.starts_with("No authentication agent is running"));
        // they ended while the prompt was open: not a failure
        assert_eq!(t(false, Some(1), "kill: (4242): No such process\nkill: (4243): No such process\n"), ("Already ended before the request was approved".to_string(), true));
        // a mixed complaint is a failure and says the first line, capped
        let (m, ok) = t(false, Some(1), &format!("kill: (4242): Operation not permitted{}\nkill: (4243): No such process", "!".repeat(400)));
        assert!(!ok && m.starts_with("pkexec kill failed: kill: (4242): Operation not permitted") && m.chars().count() < 200, "{m}");
        assert_eq!(t(false, Some(1), "").0, "pkexec kill failed: it gave no reason");
    }

    #[test]
    fn reads_own_stat() {
        let s = std::fs::read_to_string("/proc/self/stat").unwrap();
        assert!(parse_start_ticks(&s).unwrap() > 0);
    }

    #[test]
    fn kills_a_dummy_child() {
        use std::os::unix::process::ExitStatusExt;
        let mut child = std::process::Command::new("sleep").arg("600").spawn().unwrap();
        let pid = child.id() as i32;
        let ticks = parse_start_ticks(&std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap()).unwrap();
        let me = unsafe { libc::getuid() };
        let t = Target {
            name: "sleep".into(),
            procs: vec![TargetProc { pid, uid: me, start_time_ticks: ticks, name: "sleep".into(), cmdline: "sleep 600".into(), critical: None }],
        };
        let (tx, _rx) = std::sync::mpsc::channel();
        let out = execute(&t, Signal::Term, tx.clone(), egui::Context::default());
        assert!(out.ok, "{}", out.message);
        let status = child.wait().unwrap();
        assert_eq!(status.signal(), Some(libc::SIGTERM));

        // A wrong start time (recycled pid) is skipped, not killed.
        let mut child = std::process::Command::new("sleep").arg("600").spawn().unwrap();
        let pid = child.id() as i32;
        let t = Target {
            name: "sleep".into(),
            procs: vec![TargetProc { pid, uid: me, start_time_ticks: 1, name: "sleep".into(), cmdline: String::new(), critical: None }],
        };
        let out = execute(&t, Signal::Kill, tx, egui::Context::default());
        assert!(out.message.contains("already ended"), "{}", out.message);
        assert!(child.try_wait().unwrap().is_none());
        child.kill().unwrap();
        child.wait().unwrap();
    }

    #[test]
    fn messages_use_the_button_names() {
        assert_eq!(Signal::Term.label(), "End task");
        assert_eq!(Signal::Kill.label(), "Force kill");
        let t = Target {
            name: "gone".into(),
            procs: vec![TargetProc { pid: 2_000_000_000, uid: unsafe { libc::getuid() }, start_time_ticks: 1, name: "gone".into(), cmdline: String::new(), critical: None }],
        };
        let (tx, _rx) = std::sync::mpsc::channel();
        let out = execute(&t, Signal::Term, tx, egui::Context::default());
        assert_eq!(out.message, "1 already ended");
    }
}
