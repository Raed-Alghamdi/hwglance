//! Sources read on the slow interval: LACT, the SPD hubs, the NVMe drives, the Wi-Fi card,
//! the board's EC and (less often still) the Super I/O chip. Each of these talks to a device
//! or is expensive to refresh, so they are polled less often than the rest.

use crate::hwmon::{self, Chip, ChipVal};
use crate::lact::{Lact, LactVal};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

#[derive(Clone, Default)]
pub struct SlowData {
    pub chips: Vec<ChipVal>,
    pub lact: Option<LactVal>,
    pub lact_ok: bool,
    pub lact_detail: String,
    /// Counts polls, so the sampler can tell fresh values from repeats.
    pub stamp: u64,
}

/// Super I/O chips are read on every Nth slow poll (6 s at the default 2 s interval).
const SUPERIO_EVERY: u64 = 3;

pub struct SlowPoller {
    chips: Arc<Vec<Chip>>,
    lact: Lact,
    polls: u64,
    /// Last Super I/O values, repeated between their reads.
    superio: Vec<ChipVal>,
    /// Ask LACT for the NVIDIA card's extra sensors (only when an NVIDIA GPU is present).
    use_lact: bool,
}

impl SlowPoller {
    pub fn new(chips: Arc<Vec<Chip>>, use_lact: bool) -> Self {
        SlowPoller {
            chips,
            lact: Lact::new(),
            polls: 0,
            superio: Vec::new(),
            use_lact,
        }
    }

    pub fn poll(&mut self) -> SlowData {
        if self.polls.is_multiple_of(SUPERIO_EVERY) {
            self.superio = self
                .chips
                .iter()
                .filter(|c| hwmon::is_superio(&c.name))
                .map(hwmon::read_chip)
                .collect();
        }
        self.polls += 1;
        let mut chips: Vec<ChipVal> = self
            .chips
            .iter()
            .filter(|c| c.slow && !hwmon::is_superio(&c.name))
            .map(hwmon::read_chip)
            .collect();
        chips.extend(self.superio.iter().cloned());
        if !self.use_lact {
            return SlowData {
                chips,
                lact: None,
                lact_ok: false,
                lact_detail: "no NVIDIA GPU".into(),
                stamp: self.polls,
            };
        }
        match self.lact.poll() {
            Ok(v) => SlowData {
                chips,
                lact: Some(v),
                lact_ok: true,
                lact_detail: "LACT daemon answering".into(),
                stamp: self.polls,
            },
            Err(e) => SlowData {
                chips,
                lact: None,
                lact_ok: false,
                lact_detail: format!(
                    "LACT unavailable: hotspot, VRAM temperatures, fan RPM and core voltage missing ({e})"
                ),
                stamp: self.polls,
            },
        }
    }

    /// Poll forever on a background thread, publishing into `shared`.
    pub fn spawn(mut self, shared: Arc<Mutex<SlowData>>, interval: Duration) {
        thread::Builder::new()
            .name("slow".into())
            .spawn(move || {
                loop {
                    let data = self.poll();
                    if let Ok(mut g) = shared.lock() {
                        *g = data;
                    }
                    thread::sleep(interval);
                }
            })
            .expect("spawn slow poller");
    }
}
