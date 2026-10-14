//! Records the termination signals the window receives, and who sent them.
//!
//! A window started from the desktop launcher has no terminal, and SIGTERM, SIGINT, SIGHUP and
//! SIGQUIT end a process without a word. [`install`] catches them, writes a report to the crash
//! log and the journal (the signal, the sender's pid, uid and name) and then ends the process
//! the way the signal would have. SIGKILL cannot be caught.
//!
//! The handlers only copy what they were told into a pipe (everything a handler may do is
//! async-signal-safe); a thread reads the pipe and writes the report. The signals are caught
//! rather than blocked so that programs the window starts do not inherit a blocked mask.
//!
//! Linux only: it reads `/proc` and uses `si_pid`.

use std::io;
use std::sync::atomic::{AtomicI32, Ordering};

use libc::{c_int, c_void};

/// The signals recorded.
pub const SIGNALS: [c_int; 4] = [libc::SIGTERM, libc::SIGINT, libc::SIGHUP, libc::SIGQUIT];

/// The write end of the pipe the handlers use, or -1 before [`install`].
static PIPE_WRITE: AtomicI32 = AtomicI32::new(-1);

/// What a handler tells the thread. Smaller than `PIPE_BUF`, so one `write` arrives whole.
#[repr(C)]
#[derive(Clone, Copy)]
struct Record {
    signo: i32,
    code: i32,
    pid: i32,
    uid: u32,
    /// `/proc/<pid>/comm` of the sender, `comm_len` bytes of it (-1: could not be read).
    comm_len: i32,
    comm: [u8; 32],
}

const RECORD_SIZE: usize = std::mem::size_of::<Record>();

/// Catch the signals and start the thread that reports them. Call it first thing in `main`,
/// after `hwglance_proto::crash::install`. A second call does nothing.
pub fn install() -> io::Result<()> {
    if PIPE_WRITE.load(Ordering::Acquire) >= 0 {
        return Ok(());
    }
    let mut fds = [0 as c_int; 2];
    // SAFETY: fds has room for the two descriptors pipe2 returns.
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let [read_fd, write_fd] = fds;
    PIPE_WRITE.store(write_fd, Ordering::Release);
    std::thread::Builder::new()
        .name("signals".into())
        .spawn(move || reporter(read_fd))?;
    for signo in SIGNALS {
        // SAFETY: a zeroed sigaction is valid; the handler is an `extern "C"` function with the
        // SA_SIGINFO signature.
        let result = unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = handler as extern "C" fn(c_int, *mut libc::siginfo_t, *mut c_void) as usize;
            action.sa_flags = libc::SA_SIGINFO | libc::SA_RESTART;
            libc::sigemptyset(&mut action.sa_mask);
            libc::sigaction(signo, &action, std::ptr::null_mut())
        };
        if result != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

/// Runs on whichever thread the kernel picks: only async-signal-safe calls (`open`, `read`,
/// `close`, `write`) and plain memory.
extern "C" fn handler(signo: c_int, info: *mut libc::siginfo_t, _context: *mut c_void) {
    // SAFETY: errno is thread local; the kernel passes a valid siginfo with SA_SIGINFO.
    unsafe {
        let errno = libc::__errno_location();
        let saved = *errno;
        let mut record = Record { signo, code: 0, pid: 0, uid: 0, comm_len: -1, comm: [0; 32] };
        if !info.is_null() {
            record.code = (*info).si_code;
            // si_pid and si_uid are only set for a signal sent by a process (kill, sigqueue).
            if record.code <= 0 {
                record.pid = (*info).si_pid();
                record.uid = (*info).si_uid();
                record.comm_len = read_comm(record.pid, &mut record.comm);
            }
        }
        let fd = PIPE_WRITE.load(Ordering::Relaxed);
        if fd >= 0 {
            let _ = libc::write(fd, (&raw const record).cast(), RECORD_SIZE);
        }
        *errno = saved;
    }
}

/// Read `/proc/<pid>/comm` into `out` without allocating. Returns its length, or -1.
///
/// # Safety
/// Must only call async-signal-safe functions, as it runs in the handler.
unsafe fn read_comm(pid: i32, out: &mut [u8; 32]) -> i32 {
    if pid <= 0 {
        return -1;
    }
    // "/proc/" + up to 10 digits + "/comm" + NUL
    let mut path = [0u8; 32];
    let mut at = 0;
    for b in b"/proc/" {
        path[at] = *b;
        at += 1;
    }
    let mut digits = [0u8; 10];
    let mut n = pid as u32;
    let mut count = 0;
    while n > 0 {
        digits[count] = b'0' + (n % 10) as u8;
        n /= 10;
        count += 1;
    }
    while count > 0 {
        count -= 1;
        path[at] = digits[count];
        at += 1;
    }
    for b in b"/comm" {
        path[at] = *b;
        at += 1;
    }
    let fd = unsafe { libc::open(path.as_ptr().cast(), libc::O_RDONLY | libc::O_CLOEXEC) };
    if fd < 0 {
        return -1;
    }
    let got = unsafe { libc::read(fd, out.as_mut_ptr().cast(), out.len()) };
    unsafe { libc::close(fd) };
    if got <= 0 {
        return -1;
    }
    let mut len = got as usize;
    while len > 0 && out[len - 1] == b'\n' {
        len -= 1;
    }
    len as i32
}

/// Reads the records the handlers write, reports each and ends the process.
fn reporter(read_fd: c_int) {
    loop {
        let mut record = Record { signo: 0, code: 0, pid: 0, uid: 0, comm_len: -1, comm: [0; 32] };
        // SAFETY: reads at most RECORD_SIZE bytes into a Record, which has no invalid bit patterns.
        let got = unsafe { libc::read(read_fd, (&raw mut record).cast(), RECORD_SIZE) };
        if got < 0 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
            continue;
        }
        if got != RECORD_SIZE as isize {
            return;
        }
        finish(&record);
    }
}

/// Report the signal, then restore its default action and send it to ourselves: the process
/// ends with the status the signal gives (and SIGQUIT still dumps core), as without a handler.
fn finish(record: &Record) -> ! {
    let comm = (record.comm_len >= 0).then(|| String::from_utf8_lossy(&record.comm[..record.comm_len as usize]).into_owned());
    let text = describe(record, comm.as_deref());
    let fields: Vec<(&str, &str)> = text.iter().map(|(k, v)| (*k, v.as_str())).collect();
    hwglance_proto::crash::report("signal", &fields);
    // SAFETY: plain libc calls; the default action of all four signals ends the process.
    unsafe {
        libc::signal(record.signo, libc::SIG_DFL);
        libc::raise(record.signo);
    }
    // Only reached if the signal did not end the process.
    std::process::exit(128 + record.signo)
}

/// The report fields for a signal.
fn describe(record: &Record, comm: Option<&str>) -> Vec<(&'static str, String)> {
    let mut fields = vec![("signal", format!("{} ({})", name(record.signo), record.signo))];
    // A positive si_code means the kernel raised it (a closed terminal, Ctrl+\); user space
    // sends with SI_USER (0), SI_QUEUE or SI_TKILL (negative).
    if record.code > 0 || record.pid == 0 {
        fields.push(("sender", "kernel or terminal (no sending process)".to_string()));
    } else {
        fields.push(("sender pid", record.pid.to_string()));
        fields.push(("sender uid", record.uid.to_string()));
        fields.push(("sender name", comm.unwrap_or("unavailable (it may have exited)").to_string()));
    }
    fields.push(("exit", format!("default action of {}", name(record.signo))));
    fields
}

fn name(signo: c_int) -> &'static str {
    match signo {
        libc::SIGTERM => "SIGTERM",
        libc::SIGINT => "SIGINT",
        libc::SIGHUP => "SIGHUP",
        libc::SIGQUIT => "SIGQUIT",
        _ => "signal",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(signo: c_int, code: i32, pid: i32) -> Record {
        Record { signo, code, pid, uid: 1000, comm_len: -1, comm: [0; 32] }
    }

    fn text(fields: Vec<(&str, String)>) -> String {
        fields.iter().map(|(k, v)| format!("{k}: {v}\n")).collect()
    }

    #[test]
    fn a_signal_from_a_process_names_the_sender() {
        let t = text(describe(&record(libc::SIGTERM, libc::SI_USER, 4242), Some("hyprctl")));
        assert!(t.contains("signal: SIGTERM (15)\n"), "{t}");
        assert!(t.contains("sender pid: 4242\n"));
        assert!(t.contains("sender uid: 1000\n"));
        assert!(t.contains("sender name: hyprctl\n"));
        let t = text(describe(&record(libc::SIGHUP, libc::SI_USER, 7), None));
        assert!(t.contains("sender name: unavailable"), "{t}");
    }

    #[test]
    fn a_signal_from_the_kernel_has_no_sender() {
        let t = text(describe(&record(libc::SIGHUP, libc::SI_KERNEL, 0), None));
        assert!(t.contains("signal: SIGHUP (1)\n"), "{t}");
        assert!(t.contains("sender: kernel or terminal"));
        assert!(!t.contains("sender pid"));
    }

    #[test]
    fn comm_is_read_without_allocating_from_proc() {
        let mut out = [0u8; 32];
        // SAFETY: not in a handler, so any call is fine.
        let len = unsafe { read_comm(std::process::id() as i32, &mut out) };
        let expected = std::fs::read_to_string("/proc/self/comm").unwrap();
        assert_eq!(&out[..len as usize], expected.trim_end().as_bytes());
        assert_eq!(unsafe { read_comm(0, &mut out) }, -1);
        assert_eq!(unsafe { read_comm(i32::MAX, &mut out) }, -1);
    }
}
