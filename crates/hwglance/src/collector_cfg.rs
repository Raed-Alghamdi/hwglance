//! The collector's `~/.config/hwglance/config.toml`: read and change `interval_ms` while
//! keeping every other key and every comment.

use std::io;
use std::path::{Path, PathBuf};

pub const MIN_INTERVAL_MS: u32 = 100;
pub const MAX_INTERVAL_MS: u32 = 60_000;

pub fn config_path() -> PathBuf {
    crate::settings::config_dir().join("config.toml")
}

pub fn read_interval(path: &Path) -> Option<u32> {
    let text = std::fs::read_to_string(path).ok()?;
    let doc: toml_edit::DocumentMut = text.parse().ok()?;
    doc.get("interval_ms")?.as_integer().and_then(|v| u32::try_from(v).ok())
}

/// Set `interval_ms` (clamped to 100..=60000), creating the file when it does not exist.
pub fn write_interval(path: &Path, ms: u32) -> io::Result<()> {
    let ms = ms.clamp(MIN_INTERVAL_MS, MAX_INTERVAL_MS);
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e),
    };
    let mut doc: toml_edit::DocumentMut =
        text.parse().map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("config.toml: {e}")))?;
    doc["interval_ms"] = toml_edit::value(ms as i64);
    crate::settings::write_atomic(path, doc.to_string().as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("hwglance-cfg-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn creates_the_file_when_missing() {
        let p = dir("new").join("sub").join("config.toml");
        assert_eq!(read_interval(&p), None);
        write_interval(&p, 500).unwrap();
        assert_eq!(read_interval(&p), Some(500));
    }

    #[test]
    fn keeps_other_keys_and_comments() {
        let p = dir("keep").join("config.toml");
        let original = "# my collector setup\nslow_interval_ms = 4000  # slow sensors\n\n# fan names\n[labels]\n\"nct6799/fan1\" = \"Top intake\"\n\n[hide]\nids = [\"a/b\"]\n";
        std::fs::write(&p, original).unwrap();
        write_interval(&p, 250).unwrap();
        let after = std::fs::read_to_string(&p).unwrap();
        assert!(after.contains("# my collector setup"));
        assert!(after.contains("slow_interval_ms = 4000  # slow sensors"));
        assert!(after.contains("# fan names"));
        assert!(after.contains("\"nct6799/fan1\" = \"Top intake\""));
        assert!(after.contains("ids = [\"a/b\"]"));
        assert_eq!(read_interval(&p), Some(250));
        // the new key sits before the first table, so it stays a top-level key
        let toml: toml::Table = after.parse().unwrap();
        assert_eq!(toml["interval_ms"].as_integer(), Some(250));
        assert!(toml["labels"].is_table());
        // changing it again replaces it, with clamping
        write_interval(&p, 5).unwrap();
        assert_eq!(read_interval(&p), Some(MIN_INTERVAL_MS));
        assert_eq!(after.matches("interval_ms").count() - after.matches("slow_interval_ms").count(), 1);
    }

    #[test]
    fn refuses_to_overwrite_a_broken_file() {
        let p = dir("bad").join("config.toml");
        std::fs::write(&p, "interval_ms = = 3").unwrap();
        assert!(write_interval(&p, 500).is_err());
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "interval_ms = = 3");
    }
}
