//! NVIDIA GPU through NVML (loaded at run time from libnvidia-ml.so.1).

use nvml_wrapper::Nvml;
use nvml_wrapper::bitmasks::device::ThrottleReasons;
use nvml_wrapper::enum_wrappers::device::{Clock, TemperatureSensor};
use nvml_wrapper::enums::device::UsedGpuMemory;
use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::time::{Duration, Instant};

const RETRY_INIT: Duration = Duration::from_secs(10);
/// The list of processes on the GPU with their VRAM costs two driver calls (about 2 ms) and
/// changes slowly: it is asked for again after this long.
const VRAM_EVERY: Duration = Duration::from_millis(2500);

/// Device-wide values for one sample; each is `None` when NVML did not answer.
#[derive(Clone, Debug, Default)]
pub struct GpuVal {
    pub name: String,
    pub core_pct: Option<f64>,
    pub memctl_pct: Option<f64>,
    pub enc_pct: Option<f64>,
    pub dec_pct: Option<f64>,
    pub temp_c: Option<f64>,
    pub clock_core: Option<f64>,
    pub clock_mem: Option<f64>,
    pub clock_video: Option<f64>,
    pub power_w: Option<f64>,
    pub power_limit_w: Option<f64>,
    pub vram_used: Option<f64>,
    pub vram_total: Option<f64>,
    pub fan_pct: Option<f64>,
    /// Current clock event reasons: slowed down by heat / held back by the power limit.
    pub thermal_throttle: Option<bool>,
    pub power_throttle: Option<bool>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct GpuProc {
    pub sm: f32,
    pub enc: f32,
    pub dec: f32,
    pub vram: Option<u64>,
}

struct Dev {
    dev: nvml_wrapper::Device<'static>,
    name: String,
    driver: String,
    /// Newest process-utilisation timestamp seen so far.
    last_ts: u64,
    /// Processes seen on the GPU before; one missing from a later call had no activity.
    known: HashSet<u32>,
    /// VRAM per process from the last time the process lists were asked for, and when.
    vram: Vec<(u32, Option<u64>)>,
    vram_at: Option<Instant>,
}

pub struct NvmlSource {
    /// False on a machine without an NVIDIA GPU: NVML is never loaded.
    enabled: bool,
    dev: Option<Dev>,
    last_try: Option<Instant>,
    detail: String,
}

fn init_nvml() -> Result<Nvml, nvml_wrapper::error::NvmlError> {
    Nvml::builder()
        .lib_path(OsStr::new("libnvidia-ml.so.1"))
        .init()
        .or_else(|_| Nvml::init())
}

impl NvmlSource {
    pub fn new(enabled: bool) -> Self {
        let mut s = NvmlSource {
            enabled,
            dev: None,
            last_try: None,
            detail: String::new(),
        };
        if enabled {
            s.try_init();
        }
        s
    }

    fn try_init(&mut self) {
        self.last_try = Some(Instant::now());
        match init_nvml().and_then(|nvml| {
            // The library stays loaded for the life of the process.
            let nvml: &'static Nvml = Box::leak(Box::new(nvml));
            let driver = nvml.sys_driver_version()?;
            let dev = nvml.device_by_index(0)?;
            let name = dev.name()?;
            Ok(Dev {
                dev,
                name,
                driver,
                last_ts: 0,
                known: HashSet::new(),
                vram: Vec::new(),
                vram_at: None,
            })
        }) {
            Ok(d) => {
                self.detail = format!("{}, driver {}", d.name, d.driver);
                self.dev = Some(d);
            }
            Err(e) => {
                self.detail = format!("NVML unavailable: {e}");
                self.dev = None;
            }
        }
    }

    pub fn status(&self) -> (bool, String) {
        (self.dev.is_some(), self.detail.clone())
    }

    /// Device-wide readings; `None` while NVML is not available.
    pub fn sample(&mut self) -> Option<GpuVal> {
        if !self.enabled {
            return None;
        }
        if self.dev.is_none() && self.last_try.is_none_or(|t| t.elapsed() >= RETRY_INIT) {
            self.try_init();
        }
        let d = self.dev.as_ref()?;
        let reasons = d.dev.current_throttle_reasons().ok();
        let util = d.dev.utilization_rates().ok();
        let mem = d.dev.memory_info().ok();
        Some(GpuVal {
            name: d.name.clone(),
            core_pct: util.as_ref().map(|u| u.gpu as f64),
            memctl_pct: util.as_ref().map(|u| u.memory as f64),
            enc_pct: d
                .dev
                .encoder_utilization()
                .ok()
                .map(|u| u.utilization as f64),
            dec_pct: d
                .dev
                .decoder_utilization()
                .ok()
                .map(|u| u.utilization as f64),
            temp_c: d
                .dev
                .temperature(TemperatureSensor::Gpu)
                .ok()
                .map(f64::from),
            clock_core: d.dev.clock_info(Clock::Graphics).ok().map(f64::from),
            clock_mem: d.dev.clock_info(Clock::Memory).ok().map(f64::from),
            clock_video: d.dev.clock_info(Clock::Video).ok().map(f64::from),
            power_w: d.dev.power_usage().ok().map(|mw| mw as f64 / 1000.0),
            power_limit_w: d
                .dev
                .enforced_power_limit()
                .ok()
                .map(|mw| mw as f64 / 1000.0),
            vram_used: mem.as_ref().map(|m| m.used as f64),
            vram_total: mem.as_ref().map(|m| m.total as f64),
            thermal_throttle: reasons.map(|r| {
                r.intersects(
                    ThrottleReasons::SW_THERMAL_SLOWDOWN | ThrottleReasons::HW_THERMAL_SLOWDOWN,
                )
            }),
            power_throttle: reasons.map(|r| {
                r.intersects(
                    ThrottleReasons::SW_POWER_CAP | ThrottleReasons::HW_POWER_BRAKE_SLOWDOWN,
                )
            }),
            fan_pct: d.dev.fan_speed(0).ok().map(f64::from),
        })
    }

    /// Per-process GPU use since the previous call (pid -> values). NVIDIA exposes this only
    /// through NVML.
    pub fn processes(&mut self) -> HashMap<u32, GpuProc> {
        let mut out: HashMap<u32, GpuProc> = HashMap::new();
        let Some(d) = self.dev.as_mut() else {
            return out;
        };
        if let Ok(samples) = d.dev.process_utilization_stats(d.last_ts) {
            // Several samples per pid may come back; keep the newest of each.
            let mut newest: HashMap<u32, u64> = HashMap::new();
            for s in samples {
                d.last_ts = d.last_ts.max(s.timestamp);
                if newest.get(&s.pid).is_none_or(|&t| s.timestamp >= t) {
                    newest.insert(s.pid, s.timestamp);
                    out.insert(
                        s.pid,
                        GpuProc {
                            sm: s.sm_util as f32,
                            enc: s.enc_util as f32,
                            dec: s.dec_util as f32,
                            vram: None,
                        },
                    );
                }
            }
        }
        // A process with both graphics and compute contexts is listed twice with its total.
        if d.vram_at.is_none_or(|t| t.elapsed() >= VRAM_EVERY) {
            let mut running = d.dev.running_graphics_processes().unwrap_or_default();
            running.extend(d.dev.running_compute_processes().unwrap_or_default());
            d.vram = running
                .into_iter()
                .map(|p| {
                    let used = match p.used_gpu_memory {
                        UsedGpuMemory::Used(b) => Some(b),
                        _ => None,
                    };
                    (p.pid, used)
                })
                .collect();
            d.vram_at = Some(Instant::now());
        }
        for &(pid, used) in &d.vram {
            let e = out.entry(pid).or_default();
            if let Some(b) = used {
                e.vram = Some(e.vram.map_or(b, |v| v.max(b)));
            }
        }
        // A known GPU process absent from this call had no activity.
        for pid in &d.known {
            out.entry(*pid).or_default();
        }
        d.known.extend(out.keys().copied());
        out
    }

    /// Forget processes that no longer exist.
    pub fn prune(&mut self, alive: impl Fn(u32) -> bool) {
        if let Some(d) = self.dev.as_mut() {
            d.known.retain(|p| alive(*p));
        }
    }
}
