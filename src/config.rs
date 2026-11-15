//! Persistent settings model, stored as TOML at `%APPDATA%\yubi\config.toml`.
//! Every struct uses `#[serde(default)]`, so older or partial files (including the M1 layout) still load.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Smallest allowed distance between the light and deep press thresholds.
pub const MIN_THRESHOLD_GAP: f32 = 0.05;
/// Number of feature toggles counted by [`Config::enabled_count`].
pub const FEATURE_COUNT: usize = 10;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Master switch for all gestures.
    pub gestures_enabled: bool,
    pub general: General,
    /// M1 stored this table as `[gestures]`.
    #[serde(alias = "gestures")]
    pub drag: Drag,
    pub five: Five,
    pub press: Press,
    pub long_hold: LongHold,
    pub finger_hold: HoldCombos,
    pub thumb_hold: HoldCombos,
    pub corner: Corners,
    pub edge: Edges,
    pub window: WindowOps,
    pub profiles: Vec<Profile>,
    pub ring: Ring,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            gestures_enabled: true,
            general: General::default(),
            drag: Drag::default(),
            five: Five::default(),
            press: Press::default(),
            long_hold: LongHold::default(),
            finger_hold: HoldCombos::default(),
            thumb_hold: HoldCombos::default(),
            corner: Corners::default(),
            edge: Edges::default(),
            window: WindowOps::default(),
            profiles: Vec::new(),
            ring: Ring::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ThemeChoice {
    /// Black background regardless of the system setting.
    #[default]
    Black,
    System,
    Light,
}

impl ThemeChoice {
    pub const ALL: [Self; 3] = [Self::Black, Self::System, Self::Light];

    pub fn label(self) -> &'static str {
        match self {
            Self::Black => "黒（既定）",
            Self::System => "システムに合わせる",
            Self::Light => "ライト",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct General {
    pub theme: ThemeChoice,
    /// Placeholder: stored but not applied yet.
    pub start_with_windows: bool,
}

/// What a gesture does. Stored in TOML as a short string (`"task_view"`, `"keys:Alt+Tab"`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Action {
    #[default]
    None,
    TaskView,
    ShowDesktop,
    AppSwitch,
    TabSwitch,
    Find,
    MiddleClick,
    RightClick,
    RingMenu,
    Volume,
    Brightness,
    Keys(String),
}

/// The action kinds without payload, for combo boxes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionKind {
    None,
    TaskView,
    ShowDesktop,
    AppSwitch,
    TabSwitch,
    Find,
    MiddleClick,
    RightClick,
    RingMenu,
    Volume,
    Brightness,
    Keys,
}

impl ActionKind {
    pub const ALL: [Self; 12] = [
        Self::None,
        Self::TaskView,
        Self::ShowDesktop,
        Self::AppSwitch,
        Self::TabSwitch,
        Self::Find,
        Self::MiddleClick,
        Self::RightClick,
        Self::RingMenu,
        Self::Volume,
        Self::Brightness,
        Self::Keys,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::None => "なし",
            Self::TaskView => "タスクビュー",
            Self::ShowDesktop => "デスクトップ表示",
            Self::AppSwitch => "アプリ切り替え (Alt+Tab)",
            Self::TabSwitch => "タブ切り替え (Ctrl+Tab)",
            Self::Find => "検索 (Ctrl+F)",
            Self::MiddleClick => "中クリック",
            Self::RightClick => "右クリック",
            Self::RingMenu => "リングメニュー",
            Self::Volume => "音量",
            Self::Brightness => "明るさ",
            Self::Keys => "キー入力",
        }
    }
}

impl Action {
    pub fn kind(&self) -> ActionKind {
        match self {
            Self::None => ActionKind::None,
            Self::TaskView => ActionKind::TaskView,
            Self::ShowDesktop => ActionKind::ShowDesktop,
            Self::AppSwitch => ActionKind::AppSwitch,
            Self::TabSwitch => ActionKind::TabSwitch,
            Self::Find => ActionKind::Find,
            Self::MiddleClick => ActionKind::MiddleClick,
            Self::RightClick => ActionKind::RightClick,
            Self::RingMenu => ActionKind::RingMenu,
            Self::Volume => ActionKind::Volume,
            Self::Brightness => ActionKind::Brightness,
            Self::Keys(_) => ActionKind::Keys,
        }
    }

    /// Switch to `kind`; choosing the current kind keeps the value (including a key string).
    pub fn set_kind(&mut self, kind: ActionKind) {
        if self.kind() == kind {
            return;
        }
        *self = match kind {
            ActionKind::None => Self::None,
            ActionKind::TaskView => Self::TaskView,
            ActionKind::ShowDesktop => Self::ShowDesktop,
            ActionKind::AppSwitch => Self::AppSwitch,
            ActionKind::TabSwitch => Self::TabSwitch,
            ActionKind::Find => Self::Find,
            ActionKind::MiddleClick => Self::MiddleClick,
            ActionKind::RightClick => Self::RightClick,
            ActionKind::RingMenu => Self::RingMenu,
            ActionKind::Volume => Self::Volume,
            ActionKind::Brightness => Self::Brightness,
            ActionKind::Keys => Self::Keys(String::new()),
        };
    }

    /// Short Japanese text for lists.
    pub fn label(&self) -> String {
        match self {
            Self::Keys(k) if !k.is_empty() => k.clone(),
            other => other.kind().label().to_owned(),
        }
    }

    fn to_token(&self) -> String {
        match self {
            Self::None => "none".into(),
            Self::TaskView => "task_view".into(),
            Self::ShowDesktop => "show_desktop".into(),
            Self::AppSwitch => "app_switch".into(),
            Self::TabSwitch => "tab_switch".into(),
            Self::Find => "find".into(),
            Self::MiddleClick => "middle_click".into(),
            Self::RightClick => "right_click".into(),
            Self::RingMenu => "ring_menu".into(),
            Self::Volume => "volume".into(),
            Self::Brightness => "brightness".into(),
            Self::Keys(k) => format!("keys:{k}"),
        }
    }

    /// Unknown tokens become `None` on purpose: a config from a newer yubi must still load.
    fn from_token(s: &str) -> Self {
        match s {
            "task_view" => Self::TaskView,
            "show_desktop" => Self::ShowDesktop,
            "app_switch" => Self::AppSwitch,
            "tab_switch" => Self::TabSwitch,
            "find" => Self::Find,
            "middle_click" => Self::MiddleClick,
            "right_click" => Self::RightClick,
            "ring_menu" => Self::RingMenu,
            "volume" => Self::Volume,
            "brightness" => Self::Brightness,
            other => match other.strip_prefix("keys:") {
                Some(k) => Self::Keys(k.to_owned()),
                None => Self::None,
            },
        }
    }
}

impl Serialize for Action {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_token())
    }
}

impl<'de> Deserialize<'de> for Action {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(Self::from_token(&String::deserialize(d)?))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Drag {
    /// M1 name: `three_finger_drag`.
    #[serde(alias = "three_finger_drag")]
    pub enabled: bool,
    /// M1 name: `drag_fingers`.
    #[serde(alias = "drag_fingers")]
    pub fingers: u8,
    pub lift_timeout_ms: u16,
    pub keep_drag_on_release: bool,
}

impl Default for Drag {
    fn default() -> Self {
        Self {
            enabled: false,
            fingers: 3,
            lift_timeout_ms: 300,
            keep_drag_on_release: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Five {
    pub enabled: bool,
    pub pinch_in: Action,
    pub pinch_out: Action,
    pub swipe_up: Action,
    pub swipe_down: Action,
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct PressFinger {
    pub light: Action,
    pub deep: Action,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Press {
    /// Custom press feature switch (the thresholds below are always in effect).
    pub enabled: bool,
    pub light_threshold: f32,
    pub deep_threshold: f32,
    pub haptic_strength: HapticStrength,
    /// Actions for 1..=5 fingers (always 5 entries after `normalize`).
    pub fingers: Vec<PressFinger>,
}

impl Default for Press {
    fn default() -> Self {
        Self {
            enabled: false,
            light_threshold: 0.3,
            deep_threshold: 0.7,
            haptic_strength: HapticStrength::default(),
            fingers: vec![PressFinger::default(); 5],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LongHold {
    pub enabled: bool,
    pub fingers: u8,
    pub hold_ms: u16,
    pub action: Action,
}

impl Default for LongHold {
    fn default() -> Self {
        Self {
            enabled: false,
            fingers: 2,
            hold_ms: 500,
            action: Action::None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Finger {
    Thumb,
    #[default]
    Index,
    Middle,
    Ring,
    Little,
}

impl Finger {
    pub const ALL: [Self; 5] = [
        Self::Thumb,
        Self::Index,
        Self::Middle,
        Self::Ring,
        Self::Little,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Thumb => "親指",
            Self::Index => "人差し指",
            Self::Middle => "中指",
            Self::Ring => "薬指",
            Self::Little => "小指",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum HoldTrigger {
    #[default]
    Tap,
    Swipe,
}

impl HoldTrigger {
    pub const ALL: [Self; 2] = [Self::Tap, Self::Swipe];

    pub fn label(self) -> &'static str {
        match self {
            Self::Tap => "タップ",
            Self::Swipe => "スワイプ",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Combo {
    pub hold: Finger,
    pub trigger: HoldTrigger,
    /// Key combination such as `Alt+Tab`.
    pub keys: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct HoldCombos {
    pub enabled: bool,
    pub combos: Vec<Combo>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct CornerCfg {
    pub press: Action,
    pub swipe: Action,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Corners {
    pub enabled: bool,
    /// Top-left, top-right, bottom-left, bottom-right (always 4 entries after `normalize`).
    pub corners: Vec<CornerCfg>,
}

impl Default for Corners {
    fn default() -> Self {
        Self {
            enabled: false,
            corners: vec![CornerCfg::default(); 4],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum EdgeTarget {
    #[default]
    None,
    Volume,
    Brightness,
    BrushSize,
}

impl EdgeTarget {
    pub const ALL: [Self; 4] = [Self::None, Self::Volume, Self::Brightness, Self::BrushSize];

    pub fn label(self) -> &'static str {
        match self {
            Self::None => "なし",
            Self::Volume => "音量",
            Self::Brightness => "明るさ",
            Self::BrushSize => "ブラシサイズ",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Edges {
    pub enabled: bool,
    pub left: EdgeTarget,
    pub right: EdgeTarget,
    pub top: EdgeTarget,
}

impl Default for Edges {
    fn default() -> Self {
        Self {
            enabled: true,
            left: EdgeTarget::Brightness,
            right: EdgeTarget::Volume,
            top: EdgeTarget::None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowOps {
    pub enabled: bool,
    pub move_with_three_finger: bool,
    pub flick_snap: bool,
    pub corner_resize: bool,
}

impl Default for WindowOps {
    fn default() -> Self {
        Self {
            enabled: false,
            move_with_three_finger: true,
            flick_snap: true,
            corner_resize: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Profile {
    pub exe_name: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Ring {
    pub enabled: bool,
    pub fingers: u8,
    pub hold_ms: u16,
    pub items: Vec<RingItem>,
}

impl Default for Ring {
    fn default() -> Self {
        Self {
            enabled: true,
            fingers: 5,
            hold_ms: 500,
            items: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct RingItem {
    pub label: String,
    pub command: String,
}

impl Config {
    /// Clamp values into their valid ranges (deep threshold stays above light).
    pub fn normalize(&mut self) {
        self.drag.fingers = self.drag.fingers.clamp(3, 5);
        self.drag.lift_timeout_ms = self.drag.lift_timeout_ms.min(1000);
        self.long_hold.fingers = self.long_hold.fingers.clamp(1, 5);
        self.long_hold.hold_ms = self.long_hold.hold_ms.clamp(100, 2000);
        self.ring.fingers = self.ring.fingers.clamp(1, 5);
        self.ring.hold_ms = self.ring.hold_ms.clamp(100, 2000);
        self.press.fingers.resize_with(5, PressFinger::default);
        self.corner.corners.resize_with(4, CornerCfg::default);
        let p = &mut self.press;
        p.light_threshold = finite_or(p.light_threshold, 0.3).clamp(0.0, 1.0 - MIN_THRESHOLD_GAP);
        p.deep_threshold =
            finite_or(p.deep_threshold, 0.7).clamp(p.light_threshold + MIN_THRESHOLD_GAP, 1.0);
    }

    /// The per-feature enable flags, in navigation order.
    pub fn feature_flags(&self) -> [bool; FEATURE_COUNT] {
        [
            self.drag.enabled,
            self.five.enabled,
            self.press.enabled,
            self.long_hold.enabled,
            self.finger_hold.enabled,
            self.thumb_hold.enabled,
            self.corner.enabled,
            self.edge.enabled,
            self.window.enabled,
            self.ring.enabled,
        ]
    }

    /// (enabled, total) feature toggles.
    pub fn enabled_count(&self) -> (usize, usize) {
        let flags = self.feature_flags();
        (flags.iter().filter(|f| **f).count(), flags.len())
    }

    /// Trigger / action pairs for the enabled features, for the saved-assignments list.
    pub fn assignments(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        let named = |a: &Action| a.kind() != ActionKind::None;
        if self.drag.enabled {
            out.push((
                format!("{}本指ドラッグ", self.drag.fingers),
                "ドラッグ".into(),
            ));
        }
        if self.five.enabled {
            let f = &self.five;
            for (name, a) in [
                ("5本指でピンチイン", &f.pinch_in),
                ("5本指でピンチアウト", &f.pinch_out),
                ("5本指で上にスワイプ", &f.swipe_up),
                ("5本指で下にスワイプ", &f.swipe_down),
            ] {
                if named(a) {
                    out.push((name.into(), a.label()));
                }
            }
        }
        if self.press.enabled {
            for (i, f) in self.press.fingers.iter().enumerate() {
                if named(&f.light) {
                    out.push((format!("{}本指で軽く押し込む", i + 1), f.light.label()));
                }
                if named(&f.deep) {
                    out.push((format!("{}本指で深く押し込む", i + 1), f.deep.label()));
                }
            }
        }
        if self.long_hold.enabled && named(&self.long_hold.action) {
            let l = &self.long_hold;
            out.push((format!("{}本指の長押し", l.fingers), l.action.label()));
        }
        for (name, h) in [
            ("フィンガーホールド", &self.finger_hold),
            ("サムホールド", &self.thumb_hold),
        ] {
            if h.enabled {
                for c in h.combos.iter().filter(|c| !c.keys.is_empty()) {
                    out.push((
                        format!("{}（{}）+ {}", name, c.hold.label(), c.trigger.label()),
                        c.keys.clone(),
                    ));
                }
            }
        }
        if self.corner.enabled {
            const NAMES: [&str; 4] = ["左上", "右上", "左下", "右下"];
            for (name, c) in NAMES.iter().zip(&self.corner.corners) {
                if named(&c.press) {
                    out.push((format!("{name}の角を押す"), c.press.label()));
                }
                if named(&c.swipe) {
                    out.push((format!("{name}の角からスワイプ"), c.swipe.label()));
                }
            }
        }
        if self.edge.enabled {
            for (name, e) in [
                ("左端", self.edge.left),
                ("右端", self.edge.right),
                ("上端", self.edge.top),
            ] {
                if e != EdgeTarget::None {
                    out.push((format!("{name}をスライド"), e.label().into()));
                }
            }
        }
        if self.ring.enabled {
            out.push((
                format!("{}本指の長押し", self.ring.fingers),
                "リングメニュー".into(),
            ));
        }
        out
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

/// Move an unusable config file to a fresh `config.toml.bad-<unix-seconds>` name (never overwriting
/// an older backup) so the next autosave cannot destroy the user's edits.
fn move_aside(path: &Path) {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let mut n = 0u32;
    let target = loop {
        let ext = if n == 0 {
            format!("toml.bad-{secs}")
        } else {
            format!("toml.bad-{secs}-{n}")
        };
        let candidate = path.with_extension(ext);
        if !candidate.exists() {
            break candidate;
        }
        n += 1;
    };
    match fs::rename(path, &target) {
        Ok(()) => eprintln!("yubi: moved the unusable config to {}", target.display()),
        Err(e) => eprintln!("yubi: cannot move {} aside: {e}", path.display()),
    }
}

/// Load the config; a missing file yields defaults. A file that cannot be read or parsed is moved
/// aside (see `move_aside`) and defaults are used.
pub fn load(path: &Path) -> Config {
    let mut cfg = match fs::read_to_string(path) {
        Ok(text) => toml::from_str(&text).unwrap_or_else(|e| {
            eprintln!("yubi: invalid config {}: {e}", path.display());
            move_aside(path);
            Config::default()
        }),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Config::default(),
        Err(e) => {
            eprintln!("yubi: cannot read config {}: {e}", path.display());
            move_aside(path);
            Config::default()
        }
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

    fn rich() -> Config {
        let mut cfg = Config {
            gestures_enabled: false,
            ..Config::default()
        };
        cfg.general.theme = ThemeChoice::Light;
        cfg.drag.enabled = true;
        cfg.drag.fingers = 4;
        cfg.five.pinch_in = Action::TaskView;
        cfg.five.swipe_up = Action::Keys("Win+Tab".into());
        cfg.press.haptic_strength = HapticStrength::Firm;
        cfg.press.fingers[2].deep = Action::RingMenu;
        cfg.long_hold.action = Action::MiddleClick;
        cfg.finger_hold.combos.push(Combo {
            hold: Finger::Middle,
            trigger: HoldTrigger::Swipe,
            keys: "Alt+Tab".into(),
        });
        cfg.corner.corners[3].swipe = Action::Find;
        cfg.edge.top = EdgeTarget::BrushSize;
        cfg.window.flick_snap = false;
        cfg.profiles.push(Profile {
            exe_name: "code.exe".into(),
            enabled: true,
        });
        cfg.ring.items.push(RingItem {
            label: "コピー".into(),
            command: "ctrl+c".into(),
        });
        cfg
    }

    #[test]
    fn round_trip() {
        let cfg = rich();
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
    fn m1_file_loads_with_new_defaults() {
        let m1 = r#"
[gestures]
three_finger_drag = true
drag_fingers = 4

[press]
light_threshold = 0.2
deep_threshold = 0.6
haptic_strength = "firm"

[[profiles]]
exe_name = "code.exe"
enabled = true

[ring]
items = [{ label = "コピー", command = "ctrl+c" }]
"#;
        let mut cfg: Config = toml::from_str(m1).unwrap();
        cfg.normalize();
        assert!(cfg.drag.enabled);
        assert_eq!(cfg.drag.fingers, 4);
        assert_eq!(cfg.drag.lift_timeout_ms, Drag::default().lift_timeout_ms);
        assert_eq!(cfg.press.light_threshold, 0.2);
        assert_eq!(cfg.press.haptic_strength, HapticStrength::Firm);
        assert_eq!(cfg.press.fingers.len(), 5);
        assert_eq!(cfg.profiles.len(), 1);
        assert_eq!(cfg.ring.items.len(), 1);
        assert_eq!(cfg.ring.fingers, 5);
        assert_eq!(cfg.general, General::default());
        assert_eq!(cfg.general.theme, ThemeChoice::Black);
        assert!(cfg.gestures_enabled);
    }

    #[test]
    fn partial_file_fills_defaults() {
        let cfg: Config = toml::from_str("[drag]\nenabled = true\n").unwrap();
        assert!(cfg.drag.enabled);
        assert_eq!(cfg.drag.fingers, 3);
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
    fn ranges_are_clamped() {
        let mut cfg = Config::default();
        cfg.drag.fingers = 9;
        cfg.drag.lift_timeout_ms = 5000;
        cfg.long_hold.hold_ms = 5;
        cfg.press.fingers.clear();
        cfg.corner.corners.truncate(1);
        cfg.normalize();
        assert_eq!(cfg.drag.fingers, 5);
        assert_eq!(cfg.drag.lift_timeout_ms, 1000);
        assert_eq!(cfg.long_hold.hold_ms, 100);
        assert_eq!(cfg.press.fingers.len(), 5);
        assert_eq!(cfg.corner.corners.len(), 4);
        cfg.drag.fingers = 0;
        cfg.normalize();
        assert_eq!(cfg.drag.fingers, 3);
    }

    #[test]
    fn action_tokens_round_trip() {
        for kind in ActionKind::ALL {
            let mut a = Action::None;
            a.set_kind(kind);
            assert_eq!(Action::from_token(&a.to_token()), a);
        }
        assert_eq!(
            Action::from_token("keys:Ctrl+Shift+T"),
            Action::Keys("Ctrl+Shift+T".into())
        );
        assert_eq!(Action::from_token("garbage"), Action::None);
    }

    #[test]
    fn enabled_count_counts_feature_toggles() {
        let mut cfg = Config::default();
        // Defaults: edge sliders and ring menu are on.
        assert_eq!(cfg.enabled_count(), (2, FEATURE_COUNT));
        cfg.drag.enabled = true;
        cfg.window.enabled = true;
        assert_eq!(cfg.enabled_count(), (4, FEATURE_COUNT));
    }

    #[test]
    fn assignments_follow_enabled_features() {
        let mut cfg = Config::default();
        let list = cfg.assignments();
        assert!(list.contains(&("5本指の長押し".into(), "リングメニュー".into())));
        assert!(list.contains(&("左端をスライド".into(), "明るさ".into())));
        cfg.ring.enabled = false;
        cfg.edge.enabled = false;
        assert!(cfg.assignments().is_empty());
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = temp_path(name);
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn bad_files(dir: &Path) -> Vec<PathBuf> {
        fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.to_string_lossy().contains(".bad-"))
            .collect()
    }

    #[test]
    fn partial_profile_and_ring_item_load() {
        let cfg: Config = toml::from_str(
            "[[profiles]]
exe_name = \"a.exe\"
[[ring.items]]
label = \"x\"
",
        )
        .unwrap();
        assert_eq!(cfg.profiles[0].exe_name, "a.exe");
        assert!(!cfg.profiles[0].enabled);
        assert_eq!(cfg.ring.items[0].label, "x");
        assert_eq!(cfg.ring.items[0].command, "");
    }

    #[test]
    fn invalid_toml_is_moved_aside_without_overwriting_backups() {
        let dir = temp_dir("badtoml");
        let path = dir.join("config.toml");
        for _ in 0..2 {
            fs::write(&path, "this is = = not toml").unwrap();
            assert_eq!(load(&path), Config::default());
            assert!(!path.exists());
        }
        assert_eq!(bad_files(&dir).len(), 2);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn unreadable_file_is_moved_aside() {
        let dir = temp_dir("badutf");
        let path = dir.join("config.toml");
        fs::write(&path, [0xFF, 0xFE, b'a', 0, 0xC3, 0x28]).unwrap();
        assert_eq!(load(&path), Config::default());
        assert!(!path.exists());
        assert_eq!(bad_files(&dir).len(), 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_file_gives_defaults_and_no_backup() {
        let dir = temp_dir("missing");
        assert_eq!(load(&dir.join("config.toml")), Config::default());
        assert!(bad_files(&dir).is_empty());
        let _ = fs::remove_dir_all(&dir);
    }
}
