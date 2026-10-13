//! Window list from the Hyprland socket.
//!
//! The socket is asked on a thread of its own, so a Hyprland that answers slowly (or not at all)
//! never holds up sampling: [`Hypr::begin`] starts the question, [`Hypr::clients`] waits a moment
//! for the answer and otherwise uses the last one.

use serde::Deserialize;
use std::fs;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

/// Per socket operation, on the worker thread.
const TIMEOUT: Duration = Duration::from_millis(250);
/// How long sampling waits for an answer it has asked for.
const WAIT: Duration = Duration::from_millis(100);
/// An answer older than this is not used any more.
const KEEP: Duration = Duration::from_secs(10);

#[derive(Deserialize, Clone, Debug)]
pub struct Client {
    #[serde(default)]
    pub pid: i32,
    #[serde(default)]
    pub class: String,
    #[serde(default, rename = "initialClass")]
    pub initial_class: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub mapped: bool,
}

fn runtime_hypr_dir() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR")?).join("hypr"))
}

/// Candidate sockets: the one named by the environment first, then every instance, newest first.
fn candidates() -> Vec<PathBuf> {
    let Some(base) = runtime_hypr_dir() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    if let Some(sig) = std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE") {
        out.push(base.join(sig).join(".socket.sock"));
    }
    let mut found: Vec<(std::time::SystemTime, PathBuf)> = fs::read_dir(&base)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let sock = e.path().join(".socket.sock");
            let m = fs::metadata(&sock).ok()?;
            Some((m.modified().ok()?, sock))
        })
        .collect();
    found.sort_by_key(|f| std::cmp::Reverse(f.0));
    for (_, p) in found {
        if !out.contains(&p) {
            out.push(p);
        }
    }
    out
}

fn query(sock: &Path) -> Result<Vec<Client>, String> {
    let mut s = UnixStream::connect(sock).map_err(|e| e.to_string())?;
    s.set_read_timeout(Some(TIMEOUT)).ok();
    s.set_write_timeout(Some(TIMEOUT)).ok();
    s.write_all(b"j/clients").map_err(|e| e.to_string())?;
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).map_err(|e| e.to_string())?;
    serde_json::from_slice(&buf).map_err(|e| e.to_string())
}

/// What the worker found out in one round.
struct Answer {
    clients: Option<Vec<Client>>,
    status: (bool, String),
}

/// The socket handling that runs on the worker thread.
struct Finder {
    sock: Option<PathBuf>,
}

impl Finder {
    fn clients(&mut self) -> Answer {
        if let Some(sock) = &self.sock {
            match query(sock) {
                Ok(c) => return Self::ok(c),
                Err(_) => self.sock = None, // rediscover
            }
        }
        let mut last_err = "no Hyprland socket found".to_string();
        for sock in candidates() {
            match query(&sock) {
                Ok(c) => {
                    self.sock = Some(sock);
                    return Self::ok(c);
                }
                Err(e) => last_err = format!("{}: {e}", sock.display()),
            }
        }
        Answer {
            clients: None,
            status: (
                false,
                format!("Hyprland not answering, no window grouping ({last_err})"),
            ),
        }
    }

    fn ok(clients: Vec<Client>) -> Answer {
        let mapped: Vec<Client> = clients.into_iter().filter(|c| c.mapped).collect();
        Answer {
            status: (true, format!("{} windows", mapped.len())),
            clients: Some(mapped),
        }
    }
}

pub struct Hypr {
    ask: Option<Sender<()>>,
    answers: Option<Receiver<Answer>>,
    /// A question is out and its answer has not been taken yet.
    pending: bool,
    cached: Option<(Instant, Vec<Client>)>,
    status: (bool, String),
}

impl Hypr {
    pub fn new() -> Self {
        Hypr {
            ask: None,
            answers: None,
            pending: false,
            cached: None,
            status: (true, "queried while the process view is open".into()),
        }
    }

    pub fn status(&self) -> (bool, String) {
        self.status.clone()
    }

    /// Start asking Hyprland for its windows (unless the last question is still out).
    pub fn begin(&mut self) {
        if self.ask.is_none() {
            let (ask_tx, ask_rx) = mpsc::channel::<()>();
            let (ans_tx, ans_rx) = mpsc::channel::<Answer>();
            let spawned = std::thread::Builder::new().name("hypr".into()).spawn(move || {
                let mut finder = Finder { sock: None };
                while ask_rx.recv().is_ok() {
                    while ask_rx.try_recv().is_ok() {} // one answer serves every question made meanwhile
                    if ans_tx.send(finder.clients()).is_err() {
                        break;
                    }
                }
            });
            if spawned.is_err() {
                return;
            }
            self.ask = Some(ask_tx);
            self.answers = Some(ans_rx);
        }
        if !self.pending && self.ask.as_ref().is_some_and(|a| a.send(()).is_ok()) {
            self.pending = true;
        }
    }

    fn take(&mut self, a: Answer) {
        self.status = a.status;
        self.cached = a.clients.map(|c| (Instant::now(), c));
    }

    /// Mapped windows from the answer to the last [`begin`](Self::begin), waiting a moment for it;
    /// the previous answer while a new one is late. `None` when no Hyprland instance answers.
    pub fn clients(&mut self) -> Option<Vec<Client>> {
        if let Some(rx) = &self.answers {
            let mut got = Vec::new();
            if self.pending {
                match rx.recv_timeout(WAIT) {
                    Ok(a) => {
                        self.pending = false;
                        got.push(a);
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => self.pending = false,
                }
            }
            while let Ok(a) = rx.try_recv() {
                self.pending = false;
                got.push(a);
            }
            for a in got {
                self.take(a);
            }
        }
        match &self.cached {
            Some((at, c)) if at.elapsed() < KEEP => Some(c.clone()),
            _ => None,
        }
    }
}
