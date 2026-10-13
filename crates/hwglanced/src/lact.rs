//! Read-only client for the LACT daemon socket. Exactly two commands are ever sent:
//! `list_devices` and `device_stats`. LACT also accepts commands that change GPU clocks and fans.

use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

const SOCKET: &str = "/run/lactd.sock";
const TIMEOUT: Duration = Duration::from_millis(500);

#[derive(Clone, Debug, Default)]
pub struct LactVal {
    pub hotspot_c: Option<f64>,
    pub vram_c: Option<f64>,
    /// `A0`, `A1`, ... `F1` -> temperature.
    pub vram_chips: BTreeMap<String, f64>,
    pub fan_rpm: Option<f64>,
    pub voltage_v: Option<f64>,
}

pub struct Lact {
    device_id: Option<String>,
}

/// One request on a fresh connection: write a line, read a line.
fn request(cmd: &Value) -> Result<Value, String> {
    let mut s = UnixStream::connect(SOCKET).map_err(|e| format!("{SOCKET}: {e}"))?;
    s.set_read_timeout(Some(TIMEOUT)).ok();
    s.set_write_timeout(Some(TIMEOUT)).ok();
    let mut line = cmd.to_string();
    line.push('\n');
    s.write_all(line.as_bytes()).map_err(|e| e.to_string())?;
    let mut reply = String::new();
    BufReader::new(s)
        .read_line(&mut reply)
        .map_err(|e| e.to_string())?;
    let v: Value = serde_json::from_str(&reply).map_err(|e| e.to_string())?;
    if v["status"] == "ok" {
        Ok(v["data"].clone())
    } else {
        Err(format!("LACT answered {}", v["status"]))
    }
}

impl Lact {
    pub fn new() -> Self {
        Lact { device_id: None }
    }

    fn find_device(&mut self) -> Result<String, String> {
        if let Some(id) = &self.device_id {
            return Ok(id.clone());
        }
        let data = request(&json!({"command": "list_devices"}))?;
        let devices = data.as_array().cloned().unwrap_or_default();
        let nvidia = |d: &&Value| d["name"].as_str().is_some_and(|n| n.contains("NVIDIA"));
        // The NVIDIA card is the one this feeds; any other dedicated GPU only if there is none.
        let id = devices
            .iter()
            .find(nvidia)
            .or_else(|| devices.iter().find(|d| d["device_type"] == "Dedicated"))
            .and_then(|d| d["id"].as_str())
            .ok_or("no dedicated GPU listed by LACT")?
            .to_string();
        self.device_id = Some(id.clone());
        Ok(id)
    }

    pub fn poll(&mut self) -> Result<LactVal, String> {
        let result = self.poll_inner();
        if result.is_err() {
            self.device_id = None; // list again next time
        }
        result
    }

    fn poll_inner(&mut self) -> Result<LactVal, String> {
        let id = self.find_device()?;
        let data = request(&json!({"command": "device_stats", "args": {"id": id}}))?;
        Ok(parse_stats(&data))
    }
}

fn parse_stats(data: &Value) -> LactVal {
    let mut out = LactVal::default();
    if let Some(temps) = data["temps"].as_object() {
        for (name, t) in temps {
            let Some(cur) = t["current"].as_f64() else {
                continue;
            };
            match name.as_str() {
                "GPU Hotspot" => out.hotspot_c = Some(cur),
                "VRAM" => out.vram_c = Some(cur),
                n => {
                    if let Some(chip) = n.strip_prefix("VRAM Chip ") {
                        out.vram_chips.insert(chip.trim().to_string(), cur);
                    }
                }
            }
        }
    }
    out.fan_rpm = data["fan"]["speed_current"].as_f64();
    out.voltage_v = data["voltage"]["gpu"].as_f64().map(|mv| mv / 1000.0);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_stats() {
        let v = json!({
            "temps": {
                "GPU": {"current": 45.0},
                "GPU Hotspot": {"current": 54.0},
                "VRAM": {"current": 56.0},
                "VRAM Chip A0": {"current": 52.0},
                "VRAM Chip F1": {"current": 50.0}
            },
            "fan": {"speed_current": 1135},
            "voltage": {"gpu": 870}
        });
        let s = parse_stats(&v);
        assert_eq!(s.hotspot_c, Some(54.0));
        assert_eq!(s.vram_c, Some(56.0));
        assert_eq!(s.vram_chips.len(), 2);
        assert_eq!(s.fan_rpm, Some(1135.0));
        assert_eq!(s.voltage_v, Some(0.87));
    }
}
