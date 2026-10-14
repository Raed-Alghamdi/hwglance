//! A panic hook that makes sure a panic leaves a trace, and the same writer for other ways a
//! process can end.
//!
//! A panic normally prints to stderr only, and a window started from the desktop launcher has
//! no stderr. [`install`] adds a hook that writes one report per panic to the crash log
//! (`$XDG_STATE_HOME/hwglance/crash.log`, by default `~/.local/state/hwglance/crash.log`) and,
//! when stderr is not already the journal, to the journal. The previous hook still runs
//! afterwards, so the usual stderr message appears too.
//!
//! [`report`] writes a report for something that is not a panic (a startup error, a
//! termination signal) the same way, and [`note`] sends one ordinary line to the journal. Both
//! always go to the journal socket, whether or not stderr is the journal: nothing prints them
//! to stderr, and the entry carries the identifier `hwglance` either way.
//!
//! Everything here is best effort: I/O errors are ignored and nothing in the hook may panic.

use std::fmt::Write as _;
use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::os::unix::net::UnixDatagram;
use std::panic::PanicHookInfo;
use std::path::{Path, PathBuf};
use std::sync::{Once, OnceLock};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// The crash log is moved to `crash.log.1` before a report that would follow this many bytes.
const MAX_LOG_BYTES: u64 = 256 * 1024;
/// A journal datagram must fit the socket buffer, so a longer report is cut for the journal
/// (the crash log keeps it whole).
const MAX_JOURNAL_BYTES: usize = 64 * 1024;
const JOURNAL_SOCKET: &str = "/run/systemd/journal/socket";
/// Journal priorities (syslog levels): a panic, a reported error, an ordinary line.
const PRIO_CRIT: u8 = 2;
const PRIO_ERR: u8 = 3;
const PRIO_INFO: u8 = 6;

static INSTALLED: Once = Once::new();
/// Where [`report`] and [`note`] write, set by the first install.
static EVENTS: OnceLock<Events> = OnceLock::new();

struct Events {
    app: &'static str,
    dir: Option<PathBuf>,
    journal: Option<PathBuf>,
}

/// Set while a report is being written, so a panic on another thread meanwhile is not written
/// over it (the previous hook still prints it).
static BUSY: AtomicBool = AtomicBool::new(false);

/// Install the hook for `app` (the name used in reports and as the journal identifier).
/// Call it first thing in `main`. A second call does nothing.
pub fn install(app: &'static str) {
    install_to(app, state_dir(), journal_socket(), Some(PathBuf::from(JOURNAL_SOCKET)));
}

/// Like [`install`], but with the crash log directory and the journal socket given (`None`
/// leaves that destination out), for tests and tools that must not touch the real ones. The
/// panic hook and [`report`] and [`note`] all use them.
pub fn install_with(app: &'static str, dir: Option<PathBuf>, journal: Option<PathBuf>) {
    install_to(app, dir, journal.clone(), journal);
}

/// The journal socket, unless stderr already goes to the journal (`JOURNAL_STREAM` is set, as
/// for a systemd service), in which case the previous hook's message is the journal entry.
fn journal_socket() -> Option<PathBuf> {
    match std::env::var_os("JOURNAL_STREAM") {
        Some(_) => None,
        None => Some(PathBuf::from(JOURNAL_SOCKET)),
    }
}

/// `$XDG_STATE_HOME/hwglance`, else `$HOME/.local/state/hwglance`. Relative paths are ignored,
/// as the XDG specification says.
fn state_dir() -> Option<PathBuf> {
    let absolute = |name: &str| {
        std::env::var_os(name)
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
    };
    let base = absolute("XDG_STATE_HOME").or_else(|| absolute("HOME").map(|h| h.join(".local/state")))?;
    Some(base.join("hwglance"))
}

fn install_to(
    app: &'static str,
    dir: Option<PathBuf>,
    journal: Option<PathBuf>,
    events_journal: Option<PathBuf>,
) {
    INSTALLED.call_once(|| {
        let _ = EVENTS.set(Events { app, dir: dir.clone(), journal: events_journal });
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if !BUSY.swap(true, Ordering::Acquire) {
                let report = build_report(app, info);
                record(app, &report, dir.as_deref(), journal.as_deref(), PRIO_CRIT);
                BUSY.store(false, Ordering::Release);
            }
            previous(info);
        }));
    });
}

/// Write a report for an event that is not a panic to the crash log and the journal: a
/// header line naming `kind`, the app, version and pid, then one `key: value` line per field.
/// Does nothing before [`install`].
pub fn report(kind: &str, fields: &[(&str, &str)]) {
    if let Some(e) = EVENTS.get() {
        let text = format_event(&Event {
            unix_secs: now_secs(),
            app: e.app,
            version: env!("CARGO_PKG_VERSION"),
            pid: std::process::id(),
            kind,
            fields,
        });
        record(e.app, &text, e.dir.as_deref(), e.journal.as_deref(), PRIO_ERR);
    }
}

/// Send one ordinary line to the journal only (the crash log is for things that went wrong).
/// Does nothing before [`install`].
pub fn note(message: &str) {
    if let Some(e) = EVENTS.get() {
        record(e.app, message, None, e.journal.as_deref(), PRIO_INFO);
    }
}

/// The name of process `pid` from `/proc/<pid>/comm`, if it can be read (it is gone, or the
/// kernel hides it).
pub fn process_name(pid: u32) -> Option<String> {
    let raw = fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
    let name = raw.trim_end_matches('\n');
    (!name.is_empty()).then(|| name.to_string())
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

struct Event<'a> {
    unix_secs: u64,
    app: &'a str,
    version: &'a str,
    pid: u32,
    kind: &'a str,
    fields: &'a [(&'a str, &'a str)],
}

fn format_event(e: &Event<'_>) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "==== {} {} at {} ====", e.app, e.kind, iso8601(e.unix_secs));
    let _ = writeln!(out, "app: {} {}", e.app, e.version);
    let _ = writeln!(out, "pid: {}", e.pid);
    for (key, value) in e.fields {
        let _ = writeln!(out, "{key}: {value}");
    }
    out
}

fn build_report(app: &str, info: &PanicHookInfo<'_>) -> String {
    let payload = info.payload();
    let message = payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("non-string payload");
    let location = info
        .location()
        .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
        .unwrap_or_else(|| "unknown".to_string());
    let thread = std::thread::current();
    let backtrace = std::backtrace::Backtrace::force_capture().to_string();
    format_report(&Fields {
        unix_secs: now_secs(),
        app,
        version: env!("CARGO_PKG_VERSION"),
        pid: std::process::id(),
        thread: thread.name().unwrap_or("<unnamed>"),
        message,
        location: &location,
        backtrace: &backtrace,
    })
}

struct Fields<'a> {
    unix_secs: u64,
    app: &'a str,
    version: &'a str,
    pid: u32,
    thread: &'a str,
    message: &'a str,
    location: &'a str,
    backtrace: &'a str,
}

fn format_report(f: &Fields<'_>) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "==== {} panic at {} ====", f.app, iso8601(f.unix_secs));
    let _ = writeln!(out, "app: {} {}", f.app, f.version);
    let _ = writeln!(out, "pid: {}", f.pid);
    let _ = writeln!(out, "thread: {}", f.thread);
    let _ = writeln!(out, "location: {}", f.location);
    let _ = writeln!(out, "message: {}", f.message);
    let _ = writeln!(out, "backtrace:\n{}", f.backtrace.trim_end());
    out
}

/// `1970-01-01T00:00:00Z` for a count of seconds since the Unix epoch (proleptic Gregorian
/// calendar, after Howard Hinnant's `civil_from_days`).
fn iso8601(unix_secs: u64) -> String {
    let days = (unix_secs / 86_400) as i64;
    let rem = unix_secs % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3_600,
        rem % 3_600 / 60,
        rem % 60
    )
}

/// Write the report to the crash log in `dir` and to the journal at `journal`, whichever are
/// given. Errors are ignored: there is nowhere left to report them.
fn record(app: &str, report: &str, dir: Option<&Path>, journal: Option<&Path>, priority: u8) {
    if let Some(dir) = dir {
        let _ = write_crash_log(dir, report);
    }
    if let Some(socket) = journal {
        let _ = send_journal(socket, app, report, priority);
    }
}

fn write_crash_log(dir: &Path, report: &str) -> io::Result<()> {
    DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    let path = dir.join("crash.log");
    if fs::metadata(&path).is_ok_and(|m| m.len() > MAX_LOG_BYTES) {
        // rename replaces an older crash.log.1
        let _ = fs::rename(&path, dir.join("crash.log.1"));
    }
    let mut file = OpenOptions::new().create(true).append(true).mode(0o600).open(path)?;
    // One write, so reports from two processes do not interleave.
    let mut bytes = report.as_bytes().to_vec();
    bytes.push(b'\n');
    file.write_all(&bytes)
}

/// One entry in the journal's native protocol. MESSAGE spans several lines, so it uses the
/// binary form: the name, a newline, the length as 64 bits little endian, the data, a newline.
fn journal_entry(app: &str, report: &str, priority: u8) -> Vec<u8> {
    let mut end = report.len().min(MAX_JOURNAL_BYTES);
    while !report.is_char_boundary(end) {
        end -= 1;
    }
    let message = &report.as_bytes()[..end];
    let mut out = Vec::with_capacity(message.len() + 96);
    out.extend_from_slice(format!("SYSLOG_IDENTIFIER={}\nPRIORITY={priority}\nMESSAGE\n", app.replace('\n', " ")).as_bytes());
    out.extend_from_slice(&(message.len() as u64).to_le_bytes());
    out.extend_from_slice(message);
    out.push(b'\n');
    out
}

fn send_journal(socket: &Path, app: &str, report: &str, priority: u8) -> io::Result<()> {
    let sock = UnixDatagram::unbound()?;
    sock.send_to(&journal_entry(app, report, priority), socket).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hwglance-crash-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn fields<'a>() -> Fields<'a> {
        Fields {
            unix_secs: 1_791_981_296,
            app: "hwglance",
            version: "0.1.0",
            pid: 4242,
            thread: "main",
            message: "boom",
            location: "src/main.rs:7:9",
            backtrace: "   0: frame\n",
        }
    }

    #[test]
    fn timestamps_are_iso8601_utc() {
        assert_eq!(iso8601(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso8601(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(iso8601(1_709_209_199), "2024-02-29T12:19:59Z");
        assert_eq!(iso8601(1_791_981_296), "2026-10-14T12:34:56Z");
        assert_eq!(iso8601(4_102_444_799), "2099-12-31T23:59:59Z");
    }

    #[test]
    fn report_has_every_field() {
        let r = format_report(&fields());
        assert!(r.starts_with("==== hwglance panic at 2026-10-14T12:34:56Z ====\n"));
        for want in [
            "app: hwglance 0.1.0\n",
            "pid: 4242\n",
            "thread: main\n",
            "location: src/main.rs:7:9\n",
            "message: boom\n",
            "backtrace:\n   0: frame\n",
        ] {
            assert!(r.contains(want), "missing {want:?} in {r}");
        }
    }

    #[test]
    fn event_report_has_a_header_and_every_field() {
        let r = format_event(&Event {
            unix_secs: 1_791_981_296,
            app: "hwglance",
            version: "0.1.0",
            pid: 4242,
            kind: "error",
            fields: &[("error", "no display"), ("detail", "NoGlutinConfigs")],
        });
        assert_eq!(
            r,
            "==== hwglance error at 2026-10-14T12:34:56Z ====\napp: hwglance 0.1.0\npid: 4242\nerror: no display\ndetail: NoGlutinConfigs\n"
        );
    }

    #[test]
    fn process_name_reads_proc_comm() {
        assert!(process_name(std::process::id()).is_some_and(|n| !n.is_empty()));
        assert_eq!(process_name(u32::MAX), None);
    }

    #[test]
    fn crash_log_appends_and_rotates_past_256_kib() {
        let dir = scratch("rotate");
        write_crash_log(&dir, "first").unwrap();
        write_crash_log(&dir, "second").unwrap();
        assert_eq!(fs::read_to_string(dir.join("crash.log")).unwrap(), "first\nsecond\n");
        assert!(!dir.join("crash.log.1").exists());

        // Exactly the limit does not rotate; one byte more does.
        fs::write(dir.join("crash.log"), vec![b'a'; MAX_LOG_BYTES as usize]).unwrap();
        write_crash_log(&dir, "third").unwrap();
        assert!(!dir.join("crash.log.1").exists());
        fs::write(dir.join("crash.log"), vec![b'a'; MAX_LOG_BYTES as usize + 1]).unwrap();
        write_crash_log(&dir, "fourth").unwrap();
        assert_eq!(fs::metadata(dir.join("crash.log.1")).unwrap().len(), MAX_LOG_BYTES + 1);
        assert_eq!(fs::read_to_string(dir.join("crash.log")).unwrap(), "fourth\n");

        // A later rotation replaces the old .1.
        fs::write(dir.join("crash.log"), vec![b'b'; MAX_LOG_BYTES as usize + 1]).unwrap();
        write_crash_log(&dir, "fifth").unwrap();
        assert_eq!(fs::read(dir.join("crash.log.1")).unwrap()[0], b'b');
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn crash_log_directory_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("mode").join("nested");
        write_crash_log(&dir, "x").unwrap();
        assert_eq!(fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
        assert_eq!(fs::metadata(dir.join("crash.log")).unwrap().permissions().mode() & 0o777, 0o600);
        let _ = fs::remove_dir_all(dir.parent().unwrap());
    }

    #[test]
    fn journal_entry_uses_the_binary_message_form() {
        let e = journal_entry("hwglance", "line one\nline two\n", PRIO_CRIT);
        let head = b"SYSLOG_IDENTIFIER=hwglance\nPRIORITY=2\nMESSAGE\n";
        assert!(e.starts_with(head));
        let rest = &e[head.len()..];
        assert_eq!(u64::from_le_bytes(rest[..8].try_into().unwrap()), 18);
        assert_eq!(&rest[8..], b"line one\nline two\n\n");
    }

    #[test]
    fn journal_entry_cuts_long_reports_on_a_char_boundary() {
        let report = "é".repeat(MAX_JOURNAL_BYTES);
        let e = journal_entry("x", &report, PRIO_CRIT);
        let head = b"SYSLOG_IDENTIFIER=x\nPRIORITY=2\nMESSAGE\n".len();
        let len = u64::from_le_bytes(e[head..head + 8].try_into().unwrap()) as usize;
        assert!(len <= MAX_JOURNAL_BYTES);
        assert!(std::str::from_utf8(&e[head + 8..head + 8 + len]).is_ok());
    }

    #[test]
    fn journal_datagram_reaches_the_socket() {
        let dir = scratch("sock");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("journal.sock");
        let server = UnixDatagram::bind(&path).unwrap();
        record("hwglance", "report text", None, Some(&path), PRIO_INFO);
        let mut buf = [0u8; 4096];
        let n = server.recv(&mut buf).unwrap();
        assert_eq!(&buf[..n], journal_entry("hwglance", "report text", PRIO_INFO).as_slice());
        let _ = fs::remove_dir_all(&dir);
    }

    /// The only test that installs the hook (it is process-wide), and the only one that
    /// panics on purpose, so the hook sees no other panic.
    #[test]
    fn installed_hook_records_a_panic() {
        let dir = scratch("hook");
        fs::create_dir_all(&dir).unwrap();
        let sock_path = dir.join("journal.sock");
        let server = UnixDatagram::bind(&sock_path).unwrap();
        install_to("hwglance-test", Some(dir.clone()), None, Some(sock_path));
        let result = std::thread::Builder::new()
            .name("crash-test-thread".into())
            .spawn(|| {
                let n = 7;
                panic!("deliberate test panic {n}");
            })
            .unwrap()
            .join();
        assert!(result.is_err());
        let _ = std::panic::catch_unwind(|| std::panic::panic_any(5_u32));
        let log = fs::read_to_string(dir.join("crash.log")).unwrap();
        assert!(log.contains("hwglance-test panic at 20"), "{log}");
        assert!(log.contains(&format!("pid: {}\n", std::process::id())));
        assert!(log.contains("thread: crash-test-thread\n"));
        assert!(log.contains("message: deliberate test panic 7\n"));
        assert!(log.contains("location: crates/hwglance-proto/src/crash.rs:"));
        assert!(log.contains("message: non-string payload\n"));
        assert!(log.contains("backtrace:\n"));

        // report goes to the crash log and the journal socket (even with the panic hook's own
        // journal off), note to the journal only.
        report("error", &[("error", "window failed")]);
        note("started hwglance");
        let log = fs::read_to_string(dir.join("crash.log")).unwrap();
        assert!(log.contains("==== hwglance-test error at 20"), "{log}");
        assert!(log.contains("error: window failed\n"));
        assert!(!log.contains("started hwglance"));
        let mut buf = [0u8; 4096];
        let n = server.recv(&mut buf).unwrap();
        let text = String::from_utf8_lossy(&buf[..n]).into_owned();
        assert!(text.starts_with("SYSLOG_IDENTIFIER=hwglance-test\nPRIORITY=3\n"), "{text}");
        assert!(text.contains("error: window failed"));
        let n = server.recv(&mut buf).unwrap();
        let text = String::from_utf8_lossy(&buf[..n]).into_owned();
        assert!(text.starts_with("SYSLOG_IDENTIFIER=hwglance-test\nPRIORITY=6\n"), "{text}");
        assert!(text.contains("started hwglance"));
        let _ = fs::remove_dir_all(&dir);
    }
}
