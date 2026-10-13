//! Unix socket server: newline-delimited JSON, one writer thread per client so a stuck
//! client can never block sampling.

use crate::history::History;
use crate::thresholds::{Edit, Limits, MAX_EDITS};
use crate::util::now_ms;
use hwglance_proto::{ClientMsg, DaemonMsg, read_msg, write_msg};
use std::collections::VecDeque;
use std::io::{self, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

type Bytes = Arc<Vec<u8>>;

/// A client that does not accept data for this long is dropped.
const WRITE_TIMEOUT: Duration = Duration::from_secs(3);
/// Snapshots queued per client; the oldest is dropped when full.
const QUEUE_CAPACITY: usize = 2;

struct Outbox {
    queue: Mutex<VecDeque<Bytes>>,
    /// Replies and events that must not be dropped; sent before the next snapshot.
    urgent: Mutex<VecDeque<Bytes>>,
    ready: Condvar,
    closed: AtomicBool,
}

impl Outbox {
    fn new() -> Self {
        Outbox {
            queue: Mutex::new(VecDeque::new()),
            urgent: Mutex::new(VecDeque::new()),
            ready: Condvar::new(),
            closed: AtomicBool::new(false),
        }
    }

    fn push(&self, msg: Bytes) {
        let mut q = self.queue.lock().unwrap_or_else(|e| e.into_inner());
        while q.len() >= QUEUE_CAPACITY {
            q.pop_front();
        }
        q.push_back(msg);
        self.ready.notify_one();
    }

    /// Queue a message that is kept even when snapshots are being dropped (a few at most).
    fn push_urgent(&self, msg: Bytes) {
        let mut u = self.urgent.lock().unwrap_or_else(|e| e.into_inner());
        while u.len() >= 16 {
            u.pop_front();
        }
        u.push_back(msg);
        drop(u);
        let _guard = self.queue.lock();
        self.ready.notify_one();
    }

    /// Blocks until a message is queued; `None` once closed.
    fn pop(&self) -> Option<Bytes> {
        let mut q = self.queue.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if self.closed.load(Ordering::Acquire) {
                return None;
            }
            if let Some(m) = self
                .urgent
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .pop_front()
            {
                return Some(m);
            }
            if let Some(m) = q.pop_front() {
                return Some(m);
            }
            q = self.ready.wait(q).unwrap_or_else(|e| e.into_inner());
        }
    }

    fn close(&self) {
        self.closed.store(true, Ordering::Release);
        let _guard = self.queue.lock();
        self.ready.notify_all();
    }
}

struct Client {
    subscribed: AtomicBool,
    procs: AtomicBool,
    /// Wants the static process fields only when it cannot know them yet.
    slim: AtomicBool,
    /// Its window has no focus (a client that never says counts as focused).
    unfocused: AtomicBool,
    outbox: Outbox,
}

/// The latest snapshot, serialized without the process list and, when some client wanted them,
/// with it: in full (every field, command lines cut short) and slim (static fields only for
/// processes a client cannot know yet).
#[derive(Clone)]
pub struct Latest {
    pub lite: Bytes,
    pub full: Option<Bytes>,
    pub slim: Option<Bytes>,
    /// The slim version carries the static fields of every process (a client that joined can rely on it).
    pub slim_complete: bool,
}

/// Which process-carrying encodings the subscribed clients need.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Demand {
    pub full: bool,
    pub slim: bool,
}

pub struct Shared {
    clients: Mutex<Vec<Arc<Client>>>,
    latest: Mutex<Option<Latest>>,
    reset: AtomicBool,
    /// Limit changes asked for by windows, taken by the sampling loop.
    edits: Mutex<Vec<Edit>>,
    /// A slim client needs the next snapshot to carry every process' static fields.
    need_full: AtomicBool,
    history: Arc<Mutex<History>>,
    /// Wakes the sampling loop (a client asked for processes).
    kick: Sender<()>,
}

impl Shared {
    pub fn new(kick: Sender<()>, history: Arc<Mutex<History>>) -> Arc<Self> {
        Arc::new(Shared {
            clients: Mutex::new(Vec::new()),
            latest: Mutex::new(None),
            reset: AtomicBool::new(false),
            edits: Mutex::new(Vec::new()),
            need_full: AtomicBool::new(false),
            history,
            kick,
        })
    }

    /// True while any subscribed client wants processes.
    pub fn want_procs(&self) -> bool {
        let clients = self.clients.lock().unwrap_or_else(|e| e.into_inner());
        clients
            .iter()
            .any(|c| c.subscribed.load(Ordering::Relaxed) && c.procs.load(Ordering::Relaxed))
    }

    /// The encodings that subscribed clients wanting processes will be sent.
    pub fn demand(&self) -> Demand {
        let clients = self.clients.lock().unwrap_or_else(|e| e.into_inner());
        let mut d = Demand::default();
        for c in clients
            .iter()
            .filter(|c| c.subscribed.load(Ordering::Relaxed) && c.procs.load(Ordering::Relaxed))
        {
            if c.slim.load(Ordering::Relaxed) {
                d.slim = true;
            } else {
                d.full = true;
            }
        }
        d
    }

    /// True while a subscribed client that wants processes is focused (or has not said it is not).
    pub fn procs_focused(&self) -> bool {
        let clients = self.clients.lock().unwrap_or_else(|e| e.into_inner());
        clients.iter().any(|c| {
            c.subscribed.load(Ordering::Relaxed) && c.procs.load(Ordering::Relaxed) && !c.unfocused.load(Ordering::Relaxed)
        })
    }

    /// A slim client is waiting for a complete process list (without taking the flag).
    pub fn need_full_pending(&self) -> bool {
        self.need_full.load(Ordering::Acquire)
    }

    /// Whether a slim client started receiving processes since the last call.
    pub fn take_need_full(&self) -> bool {
        self.need_full.swap(false, Ordering::AcqRel)
    }

    /// Whether a client asked to reset statistics since the last call.
    pub fn take_reset(&self) -> bool {
        self.reset.swap(false, Ordering::AcqRel)
    }

    /// The limit changes asked for since the last call, oldest first.
    pub fn take_edits(&self) -> Vec<Edit> {
        std::mem::take(&mut *self.edits.lock().unwrap_or_else(|e| e.into_inner()))
    }

    fn push_edit(&self, edit: Edit) {
        let mut q = self.edits.lock().unwrap_or_else(|e| e.into_inner());
        if q.len() >= MAX_EDITS {
            q.remove(0);
        }
        q.push(edit);
    }

    pub fn publish(&self, latest: Latest) {
        *self.latest.lock().unwrap_or_else(|e| e.into_inner()) = Some(latest.clone());
        let clients = self.clients.lock().unwrap_or_else(|e| e.into_inner());
        for c in clients
            .iter()
            .filter(|c| c.subscribed.load(Ordering::Relaxed))
        {
            c.outbox.push(pick(
                &latest,
                c.procs.load(Ordering::Relaxed),
                c.slim.load(Ordering::Relaxed),
                false,
            ));
        }
    }

    /// Tell every connected window to bring a reading into view.
    pub fn broadcast_focus(&self, id: &str) {
        let mut line = Vec::new();
        if write_msg(&mut line, &DaemonMsg::Focus { id: id.to_string() }).is_err() {
            return;
        }
        let line = Arc::new(line);
        for c in self
            .clients
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
        {
            c.outbox.push_urgent(line.clone());
        }
    }

    fn remove(&self, client: &Arc<Client>) {
        client.outbox.close();
        self.clients
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|c| !Arc::ptr_eq(c, client));
    }
}

/// The version a client should get: with processes only when it wants them and they exist.
/// A client that has just subscribed (`joining`) is not sent a slim version that lacks the static
/// fields: it gets the one without processes now and the complete one with the next sample.
fn pick(latest: &Latest, wants_procs: bool, slim: bool, joining: bool) -> Bytes {
    let with_procs = match (wants_procs, slim) {
        (false, _) => None,
        (true, true) => latest.slim.as_ref().filter(|_| latest.slim_complete || !joining),
        (true, false) => latest.full.as_ref(),
    };
    with_procs.unwrap_or(&latest.lite).clone()
}

/// Bind the socket (replacing a stale file) and start accepting clients.
pub fn serve(path: &Path, shared: Arc<Shared>) -> io::Result<()> {
    if path.exists() {
        if UnixStream::connect(path).is_ok() {
            return Err(io::Error::new(
                io::ErrorKind::AddrInUse,
                "another hwglanced is already running",
            ));
        }
        std::fs::remove_file(path)?;
    }
    let listener = UnixListener::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    thread::Builder::new()
        .name("accept".into())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                let shared = shared.clone();
                let _ = thread::Builder::new()
                    .name("client".into())
                    .spawn(move || handle(stream, shared));
            }
        })?;
    Ok(())
}

fn handle(stream: UnixStream, shared: Arc<Shared>) {
    let Ok(write_half) = stream.try_clone() else {
        return;
    };
    write_half.set_write_timeout(Some(WRITE_TIMEOUT)).ok();
    let client = Arc::new(Client {
        subscribed: AtomicBool::new(false),
        procs: AtomicBool::new(false),
        slim: AtomicBool::new(false),
        unfocused: AtomicBool::new(false),
        outbox: Outbox::new(),
    });
    shared
        .clients
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(client.clone());

    let writer_client = client.clone();
    let writer = thread::Builder::new()
        .name("client-w".into())
        .spawn(move || {
            let mut w = write_half;
            while let Some(msg) = writer_client.outbox.pop() {
                if w.write_all(&msg).and_then(|_| w.flush()).is_err() {
                    break;
                }
            }
            writer_client.outbox.close();
            let _ = w.shutdown(std::net::Shutdown::Both); // unblocks the reader
        });
    if writer.is_err() {
        shared.remove(&client);
        return;
    }

    let mut reader = BufReader::new(stream);
    let mut buf = String::new();
    loop {
        match read_msg::<_, ClientMsg>(&mut reader, &mut buf) {
            Ok(Some(ClientMsg::Subscribe { procs, slim, unfocused })) => {
                client.procs.store(procs, Ordering::Relaxed);
                client.slim.store(slim, Ordering::Relaxed);
                client.unfocused.store(unfocused, Ordering::Relaxed);
                client.subscribed.store(true, Ordering::Relaxed);
                if procs && slim {
                    shared.need_full.store(true, Ordering::Release);
                }
                if let Some(latest) = shared
                    .latest
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone()
                {
                    client.outbox.push(pick(&latest, procs, slim, true));
                }
                if procs {
                    let _ = shared.kick.send(());
                }
            }
            Ok(Some(ClientMsg::SetProcs { enabled, unfocused })) => {
                client.unfocused.store(unfocused, Ordering::Relaxed);
                let was = client.procs.swap(enabled, Ordering::Relaxed);
                // Only turning the feed on owes the client a complete list; a repeat (the window
                // telling about its focus) does not.
                if enabled && !was {
                    if client.slim.load(Ordering::Relaxed) {
                        shared.need_full.store(true, Ordering::Release);
                    }
                    let _ = shared.kick.send(());
                }
            }
            Ok(Some(ClientMsg::ResetStats)) => shared.reset.store(true, Ordering::Release),
            Ok(Some(ClientMsg::SetThreshold { id, warn, crit, below })) => {
                // a limit that is not a number would poison the file: it is not accepted
                if [warn, crit].iter().flatten().all(|v| v.is_finite()) {
                    shared.push_edit(Edit::Set { id, limits: Limits { warn, crit, below } });
                }
            }
            Ok(Some(ClientMsg::ResetThreshold { id })) => shared.push_edit(Edit::Reset { id }),
            Ok(Some(ClientMsg::History { ids, seconds })) => {
                let series = match shared.history.lock() {
                    Ok(h) => h.query(now_ms(), &ids, seconds),
                    Err(_) => Vec::new(),
                };
                let mut line = Vec::new();
                if write_msg(&mut line, &DaemonMsg::History { series }).is_ok() {
                    client.outbox.push_urgent(Arc::new(line));
                }
            }
            Ok(None) => break,
            Err(e) if e.kind() == io::ErrorKind::InvalidData => {
                let msg = DaemonMsg::Error {
                    message: format!("bad message: {e}"),
                };
                let mut line = Vec::new();
                if write_msg(&mut line, &msg).is_ok() {
                    client.outbox.push_urgent(Arc::new(line));
                }
            }
            Err(_) => break,
        }
    }
    shared.remove(&client);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn latest(slim_complete: bool) -> Latest {
        Latest {
            lite: Arc::new(b"lite".to_vec()),
            full: Some(Arc::new(b"full".to_vec())),
            slim: Some(Arc::new(b"slim".to_vec())),
            slim_complete,
        }
    }

    fn text(b: Bytes) -> String {
        String::from_utf8(b.to_vec()).unwrap()
    }

    #[test]
    fn each_client_gets_the_form_it_asked_for() {
        let l = latest(false);
        assert_eq!(text(pick(&l, false, false, false)), "lite");
        assert_eq!(text(pick(&l, false, true, false)), "lite");
        assert_eq!(text(pick(&l, true, false, false)), "full");
        assert_eq!(text(pick(&l, true, true, false)), "slim");
        let mut none = latest(true);
        none.full = None;
        none.slim = None;
        assert_eq!(text(pick(&none, true, false, false)), "lite", "no processes were collected");
        assert_eq!(text(pick(&none, true, true, true)), "lite");
    }

    #[test]
    fn a_joining_slim_client_never_gets_a_slim_form_without_the_static_fields() {
        assert_eq!(text(pick(&latest(false), true, true, true)), "lite");
        assert_eq!(text(pick(&latest(true), true, true, true)), "slim");
        // a legacy client has everything in every snapshot
        assert_eq!(text(pick(&latest(false), true, false, true)), "full");
    }

    #[test]
    fn crossings_survive_a_client_reconnecting_and_follow_the_collector() {
        use hwglance_proto::{Crossing, CrossingLevel, DaemonMsg, Snapshot, Summary};
        use std::io::BufRead;
        let dir = std::env::temp_dir().join(format!("hwglance-ipc-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("daemon.sock");
        let (kick, _rx) = std::sync::mpsc::channel();
        let shared = Shared::new(kick, Arc::new(Mutex::new(History::new(0, 1))));
        serve(&path, shared.clone()).unwrap();

        let snap = |end: Option<u64>| Snapshot {
            proto: hwglance_proto::PROTO_VERSION,
            seq: 1,
            unix_ms: 1,
            stats_since_unix_ms: 0,
            interval_ms: 1000,
            history_minutes: 30,
            hostname: "h".into(),
            sensors: vec![],
            sources: vec![],
            procs: None,
            summary: Summary::default(),
            crossings: vec![Crossing {
                id: "cpu/temp/tctl".into(),
                level: CrossingLevel::Crit,
                start_ms: 100,
                peak: 91.0,
                peak_ms: 150,
                end_ms: end,
                below: false,
            }],
            limit_overrides: Default::default(),
            limits_error: None,
        };
        let publish = |s: Snapshot| {
            let latest = crate::encode(s, Demand::default(), &Default::default(), false).unwrap();
            shared.publish(latest);
        };
        let read_crossings = || -> Vec<Crossing> {
            let mut c = UnixStream::connect(&path).unwrap();
            c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            write_msg(&mut c, &ClientMsg::Subscribe { procs: false, slim: true, unfocused: false }).unwrap();
            let mut line = String::new();
            BufReader::new(c).read_line(&mut line).unwrap();
            match serde_json::from_str::<DaemonMsg>(&line).unwrap() {
                DaemonMsg::Snapshot(s) => s.crossings,
                other => panic!("{other:?}"),
            }
        };
        publish(snap(None));
        let first = read_crossings();
        assert_eq!(first.len(), 1);
        assert_eq!((first[0].start_ms, first[0].peak, first[0].end_ms), (100, 91.0, None));
        // the first client is gone; the collector still holds the list and the next client gets it
        publish(snap(Some(400)));
        let second = read_crossings();
        assert_eq!(second[0].end_ms, Some(400), "the crossing ended while nobody was connected");
        assert_eq!(second[0].start_ms, 100, "and its start is still there");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn limit_requests_from_a_window_reach_the_sampling_loop_in_order() {
        let dir = std::env::temp_dir().join(format!("hwglance-ipc-edits-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("daemon.sock");
        let (kick, _rx) = std::sync::mpsc::channel();
        let shared = Shared::new(kick, Arc::new(Mutex::new(History::new(0, 1))));
        serve(&path, shared.clone()).unwrap();
        let mut c = UnixStream::connect(&path).unwrap();
        write_msg(&mut c, &ClientMsg::SetThreshold { id: "fan/pump".into(), warn: Some(800.0), crit: None, below: true }).unwrap();
        write_msg(&mut c, &ClientMsg::ResetThreshold { id: "cpu/temp/tctl".into() }).unwrap();
        let mut got = Vec::new();
        for _ in 0..200 {
            got.extend(shared.take_edits());
            if got.len() == 2 {
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        assert_eq!(
            got,
            vec![
                Edit::Set { id: "fan/pump".into(), limits: Limits { warn: Some(800.0), crit: None, below: true } },
                Edit::Reset { id: "cpu/temp/tctl".into() },
            ]
        );
        assert!(shared.take_edits().is_empty(), "taken once");
        // a flood is capped, oldest first
        for n in 0..(MAX_EDITS + 10) {
            shared.push_edit(Edit::Reset { id: format!("r{n}") });
        }
        let all = shared.take_edits();
        assert_eq!(all.len(), MAX_EDITS);
        assert_eq!(all[0], Edit::Reset { id: "r10".into() });
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn demand_follows_subscriptions() {
        let (kick, _rx) = std::sync::mpsc::channel();
        let shared = Shared::new(kick, Arc::new(Mutex::new(History::new(0, 1))));
        let client = |procs: bool, slim: bool, subscribed: bool| {
            let c = Arc::new(Client {
                subscribed: AtomicBool::new(subscribed),
                procs: AtomicBool::new(procs),
                slim: AtomicBool::new(slim),
                unfocused: AtomicBool::new(false),
                outbox: Outbox::new(),
            });
            shared.clients.lock().unwrap().push(c);
        };
        assert_eq!(shared.demand(), Demand::default());
        client(false, true, true);
        client(true, true, false);
        assert_eq!(shared.demand(), Demand::default(), "no processes wanted, or not subscribed yet");
        client(true, true, true);
        assert_eq!(shared.demand(), Demand { full: false, slim: true });
        client(true, false, true);
        assert_eq!(shared.demand(), Demand { full: true, slim: true });
        assert!(!shared.take_need_full());
    }

    #[test]
    fn processes_count_as_focused_until_a_client_says_otherwise() {
        let (kick, _rx) = std::sync::mpsc::channel();
        let shared = Shared::new(kick, Arc::new(Mutex::new(History::new(0, 1))));
        let client = |procs: bool, unfocused: bool| {
            let c = Arc::new(Client {
                subscribed: AtomicBool::new(true),
                procs: AtomicBool::new(procs),
                slim: AtomicBool::new(true),
                unfocused: AtomicBool::new(unfocused),
                outbox: Outbox::new(),
            });
            shared.clients.lock().unwrap().push(c.clone());
            c
        };
        assert!(!shared.procs_focused(), "no client");
        let a = client(true, true);
        assert!(!shared.procs_focused(), "the only window wanting processes has no focus");
        client(false, false);
        assert!(!shared.procs_focused(), "a focused client that does not want processes does not count");
        client(true, false);
        assert!(shared.procs_focused());
        a.unfocused.store(false, Ordering::Relaxed);
        assert!(shared.procs_focused());
    }
}
