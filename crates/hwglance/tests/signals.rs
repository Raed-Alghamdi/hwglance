//! Real signals: the test starts itself as a child that installs the signal recording, sends
//! the child each termination signal and checks the report and the way the child ends.
//! It has its own `main` (see Cargo.toml) so the child runs nothing but the recording.

use std::io::{BufRead, BufReader};
use std::os::unix::net::UnixDatagram;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

const CHILD_ENV: &str = "HWGLANCE_SIGNAL_TEST_CHILD";

fn main() {
    if let Some(vars) = std::env::var_os(CHILD_ENV) {
        child(&PathBuf::from(vars));
    }
    let scratch = std::env::temp_dir().join(format!("hwglance-signals-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).unwrap();

    for (signo, name) in [(libc::SIGTERM, "SIGTERM"), (libc::SIGINT, "SIGINT"), (libc::SIGHUP, "SIGHUP"), (libc::SIGQUIT, "SIGQUIT")] {
        sent_by_this_process(&scratch, signo, name);
        println!("test {name} from this process ... ok");
    }
    sent_by_another_process(&scratch);
    println!("test SIGTERM from another process ... ok");
    let _ = std::fs::remove_dir_all(&scratch);
}

/// The child: record signals into the directory and socket the parent made, and wait.
fn child(dir: &Path) -> ! {
    // SIGQUIT would dump core, which is noise for a test.
    let none = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
    // SAFETY: plain libc call with a valid pointer.
    unsafe { libc::setrlimit(libc::RLIMIT_CORE, &none) };
    hwglance_proto::crash::install_with("hwglance-probe", Some(dir.join("state")), Some(dir.join("journal.sock")));
    hwglance::signals::install().unwrap();
    println!("ready");
    std::thread::sleep(Duration::from_secs(60));
    std::process::exit(99)
}

struct Probe {
    child: Child,
    state: PathBuf,
    journal: UnixDatagram,
}

fn start(scratch: &Path, label: &str) -> Probe {
    let dir = scratch.join(label);
    std::fs::create_dir_all(&dir).unwrap();
    let journal = UnixDatagram::bind(dir.join("journal.sock")).unwrap();
    journal.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .env(CHILD_ENV, &dir)
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(child.stdout.as_mut().unwrap()).read_line(&mut line).unwrap();
    assert_eq!(line.trim(), "ready", "the child did not start");
    Probe { child, state: dir.join("state"), journal }
}

fn wait(child: &mut Child) -> ExitStatus {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("the child did not end after the signal");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn check(mut probe: Probe, signo: i32, name: &str, sender_pid: u32, sender_name: &str) {
    let status = wait(&mut probe.child);
    // The child ends the way the signal ends a process, so a shell would report 128 + signo.
    assert_eq!(status.signal(), Some(signo), "{name}: status {status:?}");
    let log = std::fs::read_to_string(probe.state.join("crash.log")).unwrap();
    assert!(log.starts_with("==== hwglance-probe signal at 20"), "{log}");
    assert!(log.contains(&format!("pid: {}\n", probe.child.id())), "{log}");
    assert!(log.contains(&format!("signal: {name} ({signo})\n")), "{log}");
    assert!(log.contains(&format!("sender pid: {sender_pid}\n")), "{log}");
    assert!(log.contains(&format!("sender uid: {}\n", unsafe { libc::getuid() })), "{log}");
    assert!(log.contains(&format!("sender name: {sender_name}\n")), "{log}");
    let mut buf = [0u8; 4096];
    let n = probe.journal.recv(&mut buf).expect("no journal entry");
    let entry = String::from_utf8_lossy(&buf[..n]);
    assert!(entry.starts_with("SYSLOG_IDENTIFIER=hwglance-probe\n"), "{entry}");
    assert!(entry.contains(&format!("signal: {name} ({signo})")), "{entry}");
    assert!(entry.contains(&format!("sender pid: {sender_pid}")), "{entry}");
}

fn sent_by_this_process(scratch: &Path, signo: i32, name: &str) {
    let probe = start(scratch, name);
    // SAFETY: plain kill(2) on our own child.
    assert_eq!(unsafe { libc::kill(probe.child.id() as i32, signo) }, 0);
    let me = std::process::id();
    let my_name = hwglance_proto::crash::process_name(me).unwrap();
    check(probe, signo, name, me, &my_name);
}

fn sent_by_another_process(scratch: &Path) {
    let probe = start(scratch, "other");
    let mut kill = Command::new("kill").arg("-TERM").arg(probe.child.id().to_string()).spawn().unwrap();
    let sender = kill.id();
    check(probe, libc::SIGTERM, "SIGTERM", sender, "kill");
    assert!(kill.wait().unwrap().success());
}
