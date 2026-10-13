//! Files published by the root helper (`hwglance-helper`).

use crate::util::now_ms;
use hwglance_proto::{HelperMemory, HelperPower, helper_memory_path, helper_power_path};
use std::fs;
use std::time::{Duration, Instant};

/// Power readings older than this are not trusted.
const POWER_MAX_AGE_MS: u64 = 3000;
const MEMORY_RECHECK: Duration = Duration::from_secs(10);

pub struct HelperState {
    power: Option<HelperPower>,
    memory: Option<HelperMemory>,
    memory_checked: Option<Instant>,
}

impl HelperState {
    pub fn new() -> Self {
        HelperState {
            power: None,
            memory: None,
            memory_checked: None,
        }
    }

    /// Re-read the power file (every sample) and the memory file (rarely: it is written once).
    pub fn poll(&mut self) {
        self.power = fs::read_to_string(helper_power_path())
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok());
        let due = self
            .memory_checked
            .is_none_or(|t| t.elapsed() >= MEMORY_RECHECK);
        if due && self.memory.is_none() {
            self.memory = fs::read_to_string(helper_memory_path())
                .ok()
                .and_then(|t| serde_json::from_str(&t).ok());
            self.memory_checked = Some(Instant::now());
        }
    }

    /// The power reading, when it is fresh.
    pub fn power(&self) -> Option<&HelperPower> {
        self.power
            .as_ref()
            .filter(|p| now_ms().saturating_sub(p.unix_ms) < POWER_MAX_AGE_MS)
    }

    pub fn memory(&self) -> Option<&HelperMemory> {
        self.memory.as_ref().filter(|m| !m.dimms.is_empty())
    }

    pub fn status(&self) -> (bool, String) {
        if self.power().is_some() {
            let mem = if self.memory().is_some() {
                "memory speed"
            } else {
                "no memory data yet"
            };
            (true, format!("running: package and core power, {mem}"))
        } else if self.power.is_some() {
            (
                false,
                "stale: the helper stopped publishing; package power estimated from the iGPU"
                    .into(),
            )
        } else {
            (
                false,
                "not installed: package power estimated from the iGPU; no core power or memory speed".into(),
            )
        }
    }
}
