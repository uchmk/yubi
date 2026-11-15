//! Persistent settings model, stored as TOML at `%APPDATA%\yubi\config.toml`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Smallest allowed distance between the light and deep press thresholds.
pub const MIN_THRESHOLD_GAP: f32 = 0.05;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Config {
    pub gestures: Gestures,
    pub press: Press,
    pub profiles: Vec<Profile>,
    pub ring: Ring,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Gestures {
    pub three_finger_drag: bool,
    pub drag_fingers: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum HapticStrength {
    Light,
    #[default]
    Medium,
    Firm,
}

impl HapticStrength {
    pub const ALL: [Self; 3] = [Self::Light, Self::Medium, Self::Firm];

    pub fn label(self) -> &'static str {
        match self {
            Self::Light => "弱い",
            Self::Medium => "普通",
            Self::Firm => "強い",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Press {
    pub light_threshold: f32,
    pub deep_threshold: f32,
    pub haptic_strength: HapticStrength,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Profile {
    pub exe_name: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Ring {
    pub items: Vec<RingItem>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct RingItem {
    pub label: String,
    pub command: String,
}

impl Default for Gestures {
    fn default() -> Self {
        Self {
            three_finger_drag: false,
            drag_fingers: 3,
        }
    }
}

impl Default for Press {
    fn default() -> Self {
        Self {
            light_threshold: 0.3,
            deep_threshold: 0.7,
            haptic_strength: HapticStrength::default(),
        }
    }
}

impl Config {
    /// Clamp values into their valid ranges (deep threshold stays above light).
    pub fn normalize(&mut self) {
        self.gestures.drag_fingers = self.gestures.drag_fingers.clamp(3, 5);
        let p = &mut self.press;
        p.light_threshold = finite_or(p.light_threshold, 0.3).clamp(0.0, 1.0 - MIN_THRESHOLD_GAP);
        p.deep_threshold =
            finite_or(p.deep_threshold, 0.7).clamp(p.light_threshold + MIN_THRESHOLD_GAP, 1.0);
    }
}

fn finite_or(v: f32, fallback: f32) -> f32 {
    if v.is_finite() { v } else { fallback }
}

/// `%APPDATA%\yubi\config.toml` (falls back to the current directory).
pub fn default_path() -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .or_else(|| std::env::var_os("XDG_CONFIG_HOME"))
        .map(PathBuf::from)
        .unwrap_or_default();
    base.join("yubi").join("config.toml")
}

/// Load the config; a missing or unreadable file yields defaults.
/// An invalid file is moved aside to `config.toml.bad` so the next save cannot overwrite the user's edits.
pub fn load(path: &Path) -> Config {
    let mut cfg = match fs::read_to_string(path) {
        Ok(text) => toml::from_str(&text).unwrap_or_else(|e| {
            eprintln!("yubi: invalid config {}: {e}", path.display());
            let _ = fs::rename(path, path.with_extension("toml.bad"));
            Config::default()
        }),
        Err(_) => Config::default(),
    };
    cfg.normalize();
    cfg
}

/// Atomic save: write and flush a temp file next to the target, then rename over it.
pub fn save(path: &Path, cfg: &Config) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let text = toml::to_string_pretty(cfg).map_err(io::Error::other)?;
    let tmp = path.with_extension("toml.tmp");
    {
        let mut f = fs::File::create(&tmp)?;
        io::Write::write_all(&mut f, text.as_bytes())?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("yubi-test-{}-{name}", std::process::id()))
    }

    #[test]
    fn round_trip() {
        let mut cfg = Config::default();
        cfg.gestures.three_finger_drag = true;
        cfg.press.haptic_strength = HapticStrength::Firm;
        cfg.profiles.push(Profile {
            exe_name: "code.exe".into(),
            enabled: true,
        });
        cfg.ring.items.push(RingItem {
            label: "コピー".into(),
            command: "ctrl+c".into(),
        });
        let dir = temp_path("rt");
        let path = dir.join("config.toml");
        save(&path, &cfg).unwrap();
        assert_eq!(load(&path), cfg);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn missing_file_gives_defaults() {
        assert_eq!(
            load(&temp_path("missing").join("none.toml")),
            Config::default()
        );
    }

    #[test]
    fn partial_file_fills_defaults() {
        let cfg: Config = toml::from_str("[gestures]\nthree_finger_drag = true\n").unwrap();
        assert!(cfg.gestures.three_finger_drag);
        assert_eq!(cfg.gestures.drag_fingers, 3);
        assert_eq!(cfg.press, Press::default());
    }

    #[test]
    fn deep_stays_above_light() {
        let mut cfg = Config::default();
        cfg.press.light_threshold = 0.8;
        cfg.press.deep_threshold = 0.2;
        cfg.normalize();
        assert!(cfg.press.deep_threshold > cfg.press.light_threshold);
        cfg.press.light_threshold = 1.0;
        cfg.normalize();
        assert!(cfg.press.deep_threshold > cfg.press.light_threshold);
        assert!(cfg.press.deep_threshold <= 1.0);
    }

    #[test]
    fn drag_fingers_clamped() {
        let mut cfg = Config::default();
        cfg.gestures.drag_fingers = 9;
        cfg.normalize();
        assert_eq!(cfg.gestures.drag_fingers, 5);
        cfg.gestures.drag_fingers = 0;
        cfg.normalize();
        assert_eq!(cfg.gestures.drag_fingers, 3);
    }
}
