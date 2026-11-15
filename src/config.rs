//! Persistent settings model, stored as TOML at `%APPDATA%\yubi\config.toml`.
//! Every struct uses `#[serde(default)]`, so older or partial files (including the M1 layout) still load.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::engine::types::GestureId;

/// Version of the config schema this build reads and writes.
pub const SCHEMA_VERSION: u32 = 2;
/// Smallest allowed distance between the light and deep press thresholds (raw pressure).
pub const MIN_THRESHOLD_GAP: f32 = 0.02;
/// Allowed range of both press thresholds: raw pressure, which the pad reports as 0.01 to ~0.20.
pub const THRESHOLD_RANGE: (f32, f32) = (0.01, 0.20);
/// Default thresholds for one finger.
pub const DEFAULT_ONE: (f32, f32) = (0.08, 0.14);
/// Default thresholds for two to five fingers (the force is split between the contacts).
pub const DEFAULT_MULTI: (f32, f32) = (0.07, 0.12);
/// Smallest usable span between the relaxed baseline and the firm level (raw pressure scale).
pub const MIN_CALIBRATION_SPAN: f32 = 0.05;
/// Number of feature toggles counted by [`Config::enabled_count`].
pub const FEATURE_COUNT: usize = 11;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Schema of the file; a file without it is version 1 (see [`parse_text`]).
    pub schema_version: u32,
    /// Master switch for all gestures.
    pub gestures_enabled: bool,
    pub general: General,
    /// M1 stored this table as `[gestures]`.
    #[serde(alias = "gestures")]
    pub drag: Drag,
    pub four: Four,
    pub five: Five,
    pub press: Press,
    /// Per-device pressure calibration, keyed by `vvvv:pppp` (hex VID:PID). Not exported.
    pub calibration: BTreeMap<String, Calibration>,
    pub long_hold: LongHold,
    pub finger_hold: HoldCombos,
    pub thumb_hold: HoldCombos,
    pub corner: Corners,
    pub edge: Edges,
    pub window: WindowOps,
    pub sensitivity: Sensitivity,
    pub profiles: Vec<Profile>,
    pub ring: Ring,
    pub cursor: Cursor,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            gestures_enabled: true,
            general: General::default(),
            drag: Drag::default(),
            four: Four::default(),
            five: Five::default(),
            press: Press::default(),
            calibration: BTreeMap::new(),
            long_hold: LongHold::default(),
            finger_hold: HoldCombos::default(),
            thumb_hold: HoldCombos::default(),
            corner: Corners::default(),
            edge: Edges::default(),
            window: WindowOps::default(),
            sensitivity: Sensitivity::default(),
            profiles: Vec::new(),
            ring: Ring::default(),
            cursor: Cursor::default(),
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct General {
    pub theme: ThemeChoice,
    /// Placeholder: stored but not applied yet.
    pub start_with_windows: bool,
    /// Global shortcut that pauses and resumes yubi (a key combo such as `Ctrl+Alt+Y`; empty = none).
    pub pause_shortcut: String,
    /// Pause yubi by itself while a full-screen exclusive app is in front.
    pub pause_in_fullscreen: bool,
}

/// The pause shortcut a fresh config starts with.
pub const DEFAULT_PAUSE_SHORTCUT: &str = "Ctrl+Alt+Y";

impl Default for General {
    fn default() -> Self {
        Self {
            theme: ThemeChoice::default(),
            start_with_windows: false,
            pause_shortcut: DEFAULT_PAUSE_SHORTCUT.to_owned(),
            pause_in_fullscreen: false,
        }
    }
}

/// What a gesture does. Stored in TOML as a short string (`"task_view"`, `"keys:Alt+Tab"`,
/// `"volume+2"`).
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
    /// Change the master volume by this many percent points (negative = down).
    VolumeStep(i8),
    /// Change the display brightness by this many percent points (negative = down).
    BrightnessStep(i8),
    ToggleMute,
    DesktopPrev,
    DesktopNext,
    Keys(String),
}

/// Step the picker starts with when the user switches to a volume or brightness action.
pub const DEFAULT_VOLUME_STEP: i8 = 2;
pub const DEFAULT_BRIGHTNESS_STEP: i8 = 5;
/// Largest step a volume / brightness action may carry.
pub const MAX_STEP: i8 = 50;

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
    VolumeStep,
    BrightnessStep,
    ToggleMute,
    DesktopPrev,
    DesktopNext,
    Keys,
}

impl ActionKind {
    pub const ALL: [Self; 15] = [
        Self::None,
        Self::TaskView,
        Self::ShowDesktop,
        Self::AppSwitch,
        Self::TabSwitch,
        Self::Find,
        Self::MiddleClick,
        Self::RightClick,
        Self::RingMenu,
        Self::VolumeStep,
        Self::BrightnessStep,
        Self::ToggleMute,
        Self::DesktopPrev,
        Self::DesktopNext,
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
            Self::VolumeStep => "音量を変える",
            Self::BrightnessStep => "明るさを変える",
            Self::ToggleMute => "ミュートの切り替え",
            Self::DesktopPrev => "前のデスクトップ",
            Self::DesktopNext => "次のデスクトップ",
            Self::Keys => "キー入力",
        }
    }
}

/// What parsing one action token found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenParse {
    Ok(Action),
    /// A token of an older yubi whose meaning was ambiguous (`volume`, `brightness`).
    Legacy,
    Unknown,
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
            Self::VolumeStep(_) => ActionKind::VolumeStep,
            Self::BrightnessStep(_) => ActionKind::BrightnessStep,
            Self::ToggleMute => ActionKind::ToggleMute,
            Self::DesktopPrev => ActionKind::DesktopPrev,
            Self::DesktopNext => ActionKind::DesktopNext,
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
            ActionKind::VolumeStep => Self::VolumeStep(DEFAULT_VOLUME_STEP),
            ActionKind::BrightnessStep => Self::BrightnessStep(DEFAULT_BRIGHTNESS_STEP),
            ActionKind::ToggleMute => Self::ToggleMute,
            ActionKind::DesktopPrev => Self::DesktopPrev,
            ActionKind::DesktopNext => Self::DesktopNext,
            ActionKind::Keys => Self::Keys(String::new()),
        };
    }

    /// Short Japanese text for lists.
    pub fn label(&self) -> String {
        match self {
            Self::Keys(k) if !k.is_empty() => k.clone(),
            Self::VolumeStep(n) => format!("音量 {n:+}"),
            Self::BrightnessStep(n) => format!("明るさ {n:+}"),
            other => other.kind().label().to_owned(),
        }
    }

    pub fn to_token(&self) -> String {
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
            Self::VolumeStep(n) => format!("volume{n:+}"),
            Self::BrightnessStep(n) => format!("brightness{n:+}"),
            Self::ToggleMute => "mute".into(),
            Self::DesktopPrev => "desktop_prev".into(),
            Self::DesktopNext => "desktop_next".into(),
            Self::Keys(k) => format!("keys:{k}"),
        }
    }

    /// `+2` / `-5`: the sign is required, so the legacy bare tokens never match.
    fn parse_step(rest: &str) -> Option<i8> {
        let signed = rest.starts_with('+') || rest.starts_with('-');
        let n: i8 = rest.parse().ok().filter(|_| signed)?;
        (-MAX_STEP..=MAX_STEP).contains(&n).then_some(n)
    }

    pub fn parse_token(s: &str) -> TokenParse {
        let a = match s {
            "none" => Self::None,
            "task_view" => Self::TaskView,
            "show_desktop" => Self::ShowDesktop,
            "app_switch" => Self::AppSwitch,
            "tab_switch" => Self::TabSwitch,
            "find" => Self::Find,
            "middle_click" => Self::MiddleClick,
            "right_click" => Self::RightClick,
            "ring_menu" => Self::RingMenu,
            "mute" => Self::ToggleMute,
            "desktop_prev" => Self::DesktopPrev,
            "desktop_next" => Self::DesktopNext,
            "volume" | "brightness" => return TokenParse::Legacy,
            other => {
                if let Some(k) = other.strip_prefix("keys:") {
                    Self::Keys(k.to_owned())
                } else if let Some(n) = other.strip_prefix("volume").and_then(Self::parse_step) {
                    Self::VolumeStep(n)
                } else if let Some(n) = other.strip_prefix("brightness").and_then(Self::parse_step)
                {
                    Self::BrightnessStep(n)
                } else {
                    return TokenParse::Unknown;
                }
            }
        };
        TokenParse::Ok(a)
    }

    /// Unknown and legacy tokens become `None` on purpose: a config from a newer or older yubi
    /// must still load. [`parse_text`] reports them.
    #[allow(dead_code)] // used by the edge recognizer that comes later, and by tests
    pub fn from_token(s: &str) -> Self {
        match Self::parse_token(s) {
            TokenParse::Ok(a) => a,
            TokenParse::Legacy | TokenParse::Unknown => Self::None,
        }
    }
}

impl Serialize for Action {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_token())
    }
}

/// An action value as written: a string, or something of the wrong type (replaced, not fatal).
#[derive(Deserialize)]
#[serde(untagged)]
enum TokenWire {
    Text(String),
    Other(serde::de::IgnoredAny),
}

/// A finger count as written: an integer, or something of the wrong type (the default is used).
#[derive(Deserialize)]
#[serde(untagged)]
enum CountWire {
    Int(i64),
    Other(serde::de::IgnoredAny),
}

/// Read a finger count leniently: a number out of the `u8` range saturates (the range check is
/// [`Config::normalize`]'s job), anything else falls back to `default` with a note.
fn lenient_count<'de, D: Deserializer<'de>>(d: D, what: &str, default: u8) -> Result<u8, D::Error> {
    Ok(match CountWire::deserialize(d)? {
        CountWire::Int(n) => n.clamp(0, i64::from(u8::MAX)) as u8,
        CountWire::Other(_) => {
            scan_note(|s| {
                s.bad.push(format!(
                    "{what}の値が整数ではないため、既定値 {default} を使います"
                ));
            });
            default
        }
    })
}

/// A cursor speed as written: `None` when absent or not an integer (then Windows' value is left
/// alone, with a note); a number out of range saturates and `Config::normalize` clamps it.
fn cursor_speed<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u8>, D::Error> {
    Ok(match CountWire::deserialize(d)? {
        CountWire::Int(n) => {
            let stored = n.clamp(0, i64::from(u8::MAX)) as u8;
            if i64::from(stored) != n {
                scan_note(|s| s.raw_speeds.push((stored, n)));
            }
            Some(stored)
        }
        CountWire::Other(_) => {
            scan_note(|s| {
                s.bad.push(
                    "カーソルの速度の値が整数ではないため、Windows の設定のままにします".to_owned(),
                );
            });
            None
        }
    })
}

fn drag_fingers<'de, D: Deserializer<'de>>(d: D) -> Result<u8, D::Error> {
    lenient_count(d, "ドラッグの指の本数", Drag::default().fingers)
}

fn long_hold_fingers<'de, D: Deserializer<'de>>(d: D) -> Result<u8, D::Error> {
    lenient_count(d, "長押しの指の本数", LongHold::default().fingers)
}

fn ring_fingers<'de, D: Deserializer<'de>>(d: D) -> Result<u8, D::Error> {
    lenient_count(d, "リングメニューの指の本数", Ring::default().fingers)
}

impl<'de> Deserialize<'de> for Action {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let token = match TokenWire::deserialize(d)? {
            TokenWire::Text(t) => t,
            TokenWire::Other(_) => {
                scan_note(|s| {
                    s.bad
                        .push("動作の値が文字列ではないため「なし」にしました".to_owned());
                });
                return Ok(Self::None);
            }
        };
        Ok(match Self::parse_token(&token) {
            TokenParse::Ok(a) => a,
            TokenParse::Legacy => {
                scan_note(|s| s.legacy.push(token));
                Self::None
            }
            TokenParse::Unknown => {
                scan_note(|s| s.unknown.push(token));
                Self::None
            }
        })
    }
}

/// What the deserializers noticed while one text was parsed (see [`parse_text`]).
#[derive(Debug, Default)]
struct Scan {
    legacy: Vec<String>,
    unknown: Vec<String>,
    brush_size: usize,
    /// Ready-made notes for values of the wrong type that were replaced by their default.
    bad: Vec<String>,
    /// Whole-number sensitivity values the file held that do not fit the stored type, by their
    /// Japanese name: the clamp report shows these, not the saturated stored value.
    raw_nums: Vec<(&'static str, f64)>,
    /// Cursor speeds from the file that saturated to `u8` (stored value, value as written), in
    /// file order: the clamp report shows what was written.
    raw_speeds: Vec<(u8, i64)>,
}

thread_local! {
    static SCAN: std::cell::RefCell<Scan> = std::cell::RefCell::default();
}

fn scan_note(f: impl FnOnce(&mut Scan)) {
    SCAN.with(|s| f(&mut s.borrow_mut()));
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Drag {
    /// M1 name: `three_finger_drag`.
    #[serde(alias = "three_finger_drag")]
    pub enabled: bool,
    /// M1 name: `drag_fingers`.
    #[serde(alias = "drag_fingers", deserialize_with = "drag_fingers")]
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

/// Four-finger gestures. On by default; the swipes follow Windows' own four-finger gestures
/// (content follows the fingers: swipe left shows the next desktop).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Four {
    pub enabled: bool,
    pub tap: Action,
    pub swipe_left: Action,
    pub swipe_right: Action,
    pub swipe_up: Action,
    pub swipe_down: Action,
    pub pinch_in: Action,
    pub pinch_out: Action,
}

impl Default for Four {
    fn default() -> Self {
        Self {
            enabled: true,
            tap: Action::None,
            swipe_left: Action::DesktopNext,
            swipe_right: Action::DesktopPrev,
            swipe_up: Action::TaskView,
            swipe_down: Action::ShowDesktop,
            pinch_in: Action::None,
            pinch_out: Action::None,
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

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct PressFinger {
    /// This finger count's press is switched on. A file from before this field existed (the count
    /// was active whenever it had a binding) reads as on.
    #[serde(default = "yes")]
    pub enabled: bool,
    pub light: Action,
    pub deep: Action,
    /// Own thresholds for this count; both or neither (`normalize` pairs them). `None` means
    /// "same as one finger" ([`Press::thresholds`]). Ignored for one finger.
    pub light_threshold: Option<f32>,
    pub deep_threshold: Option<f32>,
}

impl PressFinger {
    /// A fresh entry for `fingers` fingers: two or more start with their own, lower thresholds.
    pub fn for_count(fingers: usize) -> Self {
        let own = fingers >= 2;
        Self {
            light_threshold: own.then_some(DEFAULT_MULTI.0),
            deep_threshold: own.then_some(DEFAULT_MULTI.1),
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Press {
    /// Custom press feature switch. Thresholds are raw pressure (0.01 to 0.20); this pair is the
    /// one-finger one and the fallback of the counts that share it.
    pub enabled: bool,
    pub light_threshold: f32,
    pub deep_threshold: f32,
    pub haptic_strength: HapticStrength,
    /// A short tick when a light / deep press is recognised.
    pub haptic_ticks: bool,
    /// Actions for 1..=5 fingers (always 5 entries after `normalize`).
    pub fingers: Vec<PressFinger>,
}

impl Default for Press {
    fn default() -> Self {
        Self {
            enabled: false,
            light_threshold: DEFAULT_ONE.0,
            deep_threshold: DEFAULT_ONE.1,
            haptic_strength: HapticStrength::default(),
            haptic_ticks: true,
            fingers: (1..=5).map(PressFinger::for_count).collect(),
        }
    }
}

impl Press {
    /// Light and deep thresholds (raw pressure) that apply to `fingers` fingers.
    pub fn thresholds(&self, fingers: usize) -> (f32, f32) {
        let own = (fingers >= 2)
            .then(|| self.fingers.get(fingers - 1))
            .flatten()
            .and_then(|f| Some((f.light_threshold?, f.deep_threshold?)));
        own.unwrap_or((self.light_threshold, self.deep_threshold))
    }

    /// Whether the press of `fingers` fingers can fire: the global switch and the count's own
    /// switch are on. No calibration is needed.
    pub fn count_active(&self, fingers: usize) -> bool {
        self.enabled
            && fingers
                .checked_sub(1)
                .and_then(|i| self.fingers.get(i))
                .is_some_and(|f| f.enabled)
    }
}

/// Hundredths of a raw pressure value (`0.07` -> 7), so the thresholds never drift as floats.
fn centi(v: f32) -> i32 {
    (f64::from(v) * 100.0).round() as i32
}

fn from_centi(c: i32) -> f32 {
    c as f32 / 100.0
}

/// Pair a light / deep threshold: both inside [`THRESHOLD_RANGE`], at least the gap apart, and in
/// whole hundredths. Light stops two hundredths below the top so deep still fits.
pub fn fit_pair(light: f32, deep: f32) -> (f32, f32) {
    let (lo, hi) = (centi(THRESHOLD_RANGE.0), centi(THRESHOLD_RANGE.1));
    let gap = centi(MIN_THRESHOLD_GAP);
    let l = centi(light).clamp(lo, hi - gap);
    let d = centi(deep).clamp(l + gap, hi);
    (from_centi(l), from_centi(d))
}

/// Pair for `normalize`: when either value is outside the range (an old calibrated-scale setting,
/// a typo, NaN) the whole pair is replaced by its default, so a half-valid pair never survives
/// next to a default; a pair that is in range is kept.
fn clamp_pair(light: f32, deep: f32, default: (f32, f32)) -> (f32, f32) {
    let (lo, hi) = (centi(THRESHOLD_RANGE.0), centi(THRESHOLD_RANGE.1));
    let ok = |v: f32| v.is_finite() && (lo..=hi).contains(&centi(v));
    if ok(light) && ok(deep) {
        fit_pair(light, deep)
    } else {
        fit_pair(default.0, default.1)
    }
}

/// Per-device pressure measurement on the raw pressure scale: a helper for choosing thresholds and
/// for the marks on the live meter; it does not gate anything.
/// `baseline` is the relaxed touch, `f1`..`f5` the firm level with one to five fingers
/// (the pad splits its force between contacts, so each count has its own).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Calibration {
    pub baseline: f32,
    pub f1: f32,
    pub f2: Option<f32>,
    pub f3: Option<f32>,
    pub f4: Option<f32>,
    pub f5: Option<f32>,
    /// `YYYY-MM-DD` (UTC) of the last change.
    pub date: String,
}

/// Round to 0.01 so the stored value reads as typed (no `0.15000001`).
pub fn q01(v: f32) -> f32 {
    from_centi(centi(v))
}

impl Calibration {
    /// The firm level for `fingers` (1..=5), if calibrated.
    pub fn firm(&self, fingers: usize) -> Option<f32> {
        match fingers {
            1 => Some(self.f1),
            2 => self.f2,
            3 => self.f3,
            4 => self.f4,
            5 => self.f5,
            _ => None,
        }
    }

    /// True when `baseline` and the firm level for `fingers` form a usable span.
    #[cfg(test)]
    pub fn usable(&self, fingers: usize) -> bool {
        self.firm(fingers)
            .is_some_and(|f| span_ok(self.baseline, f))
    }
}

fn span_ok(baseline: f32, firm: f32) -> bool {
    baseline.is_finite()
        && firm.is_finite()
        && (0.0..=1.0).contains(&baseline)
        && firm <= 1.0
        && firm - baseline >= MIN_CALIBRATION_SPAN - 1e-4
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LongHold {
    pub enabled: bool,
    #[serde(deserialize_with = "long_hold_fingers")]
    pub fingers: u8,
    pub action: Action,
}

impl Default for LongHold {
    fn default() -> Self {
        Self {
            enabled: false,
            fingers: 2,
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

    /// Spelling in gesture ids.
    pub fn token(self) -> &'static str {
        match self {
            Self::Thumb => "thumb",
            Self::Index => "index",
            Self::Middle => "middle",
            Self::Ring => "ring",
            Self::Little => "little",
        }
    }

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

    /// Spelling in gesture ids.
    pub fn token(self) -> &'static str {
        match self {
            Self::Tap => "tap",
            Self::Swipe => "swipe",
        }
    }

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

/// Which keys a seek slider sends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeekPreset {
    /// `L` forward, `J` back (10 s; works without the player focused).
    Youtube,
    /// `Right` forward, `Left` back (5 s).
    General,
}

/// Keys and step distance of a user-defined edge slider.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CustomEdge {
    /// Key combination sent for one step forward (for example `Ctrl+Tab`).
    pub forward: String,
    /// Key combination sent for one step back.
    pub back: String,
    /// Travel along the edge that makes one step.
    pub step_mm: f32,
}

impl Default for CustomEdge {
    fn default() -> Self {
        Self {
            forward: String::new(),
            back: String::new(),
            step_mm: Sensitivity::default().edge_detent_mm,
        }
    }
}

/// What sliding a finger along one pad edge does.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum EdgeTarget {
    #[default]
    None,
    Volume,
    Brightness,
    Scroll,
    /// Ctrl + mouse wheel.
    Zoom,
    Seek(SeekPreset),
    UndoRedo,
    Tab,
    Custom(CustomEdge),
}

/// The edge targets without payload, for combo boxes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeKind {
    None,
    Volume,
    Brightness,
    Scroll,
    Zoom,
    SeekYoutube,
    SeekGeneral,
    UndoRedo,
    Tab,
    Custom,
}

impl EdgeKind {
    pub const ALL: [Self; 10] = [
        Self::None,
        Self::Volume,
        Self::Brightness,
        Self::Scroll,
        Self::Zoom,
        Self::SeekYoutube,
        Self::SeekGeneral,
        Self::UndoRedo,
        Self::Tab,
        Self::Custom,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::None => "なし",
            Self::Volume => "音量",
            Self::Brightness => "明るさ",
            Self::Scroll => "スクロール",
            Self::Zoom => "ズーム",
            Self::SeekYoutube => "動画の早送り・巻き戻し (YouTube: L/J)",
            Self::SeekGeneral => "動画の早送り・巻き戻し (一般: →/←)",
            Self::UndoRedo => "元に戻す／やり直し",
            Self::Tab => "タブの切り替え",
            Self::Custom => "カスタム",
        }
    }
}

/// One output of an edge step (see `EdgeTarget::step`).
#[derive(Debug, Clone, PartialEq)]
pub enum EdgeOut {
    /// Run a normal action (key combination, volume step, ...).
    Act(Action),
    /// Mouse wheel notches (positive = up / away from the user).
    Wheel(i32),
    /// Mouse wheel notches with Ctrl held (zoom).
    CtrlWheel(i32),
}

impl EdgeTarget {
    pub fn kind(&self) -> EdgeKind {
        match self {
            Self::None => EdgeKind::None,
            Self::Volume => EdgeKind::Volume,
            Self::Brightness => EdgeKind::Brightness,
            Self::Scroll => EdgeKind::Scroll,
            Self::Zoom => EdgeKind::Zoom,
            Self::Seek(SeekPreset::Youtube) => EdgeKind::SeekYoutube,
            Self::Seek(SeekPreset::General) => EdgeKind::SeekGeneral,
            Self::UndoRedo => EdgeKind::UndoRedo,
            Self::Tab => EdgeKind::Tab,
            Self::Custom(_) => EdgeKind::Custom,
        }
    }

    /// Switch to `kind`; choosing the current kind keeps the value (including custom keys).
    pub fn set_kind(&mut self, kind: EdgeKind) {
        if self.kind() == kind {
            return;
        }
        *self = match kind {
            EdgeKind::None => Self::None,
            EdgeKind::Volume => Self::Volume,
            EdgeKind::Brightness => Self::Brightness,
            EdgeKind::Scroll => Self::Scroll,
            EdgeKind::Zoom => Self::Zoom,
            EdgeKind::SeekYoutube => Self::Seek(SeekPreset::Youtube),
            EdgeKind::SeekGeneral => Self::Seek(SeekPreset::General),
            EdgeKind::UndoRedo => Self::UndoRedo,
            EdgeKind::Tab => Self::Tab,
            EdgeKind::Custom => Self::Custom(CustomEdge::default()),
        };
    }

    pub fn label(&self) -> &'static str {
        self.kind().label()
    }

    /// What one detent step does. `forward` is the direction of travel (up along a side edge or right
    /// along the top edge counts as forward, down or left as back). Empty for `None` and for a custom slider
    /// whose key for that direction is empty or invalid.
    pub fn step(&self, forward: bool) -> Vec<EdgeOut> {
        let keys = |k: &str| vec![EdgeOut::Act(Action::Keys(k.to_owned()))];
        let sign = if forward { 1 } else { -1 };
        match self {
            Self::None => Vec::new(),
            Self::Volume => vec![EdgeOut::Act(Action::VolumeStep(
                DEFAULT_VOLUME_STEP * sign as i8,
            ))],
            Self::Brightness => vec![EdgeOut::Act(Action::BrightnessStep(
                DEFAULT_BRIGHTNESS_STEP * sign as i8,
            ))],
            Self::Scroll => vec![EdgeOut::Wheel(-sign)],
            Self::Zoom => vec![EdgeOut::CtrlWheel(-sign)],
            Self::Seek(SeekPreset::Youtube) => keys(if forward { "L" } else { "J" }),
            Self::Seek(SeekPreset::General) => keys(if forward { "Right" } else { "Left" }),
            Self::UndoRedo => keys(if forward { "Ctrl+Y" } else { "Ctrl+Z" }),
            Self::Tab => keys(if forward {
                "Ctrl+Tab"
            } else {
                "Ctrl+Shift+Tab"
            }),
            Self::Custom(c) => {
                let k = if forward { &c.forward } else { &c.back };
                if k.trim().parse::<crate::os::keys::KeyCombo>().is_ok() {
                    keys(k.trim())
                } else {
                    Vec::new()
                }
            }
        }
    }

    /// Travel per step in mm, given the global detent distance.
    pub fn step_mm(&self, detent_mm: f32) -> f32 {
        match self {
            Self::Custom(c) => c.step_mm,
            _ => detent_mm,
        }
    }
}

/// Wire form: a name for every target but `Custom`, which is a table `{ custom = { ... } }`.
/// `brush_size` (removed) and unknown names load as `None`.
#[derive(Deserialize)]
#[serde(untagged)]
enum EdgeWire {
    Name(String),
    Custom {
        custom: CustomEdge,
    },
    /// Any other type (a number, a list ...): replaced by `none` with a note.
    Other(serde::de::IgnoredAny),
}

#[derive(Serialize)]
struct CustomWire<'a> {
    custom: &'a CustomEdge,
}

impl EdgeTarget {
    fn name(&self) -> Option<&'static str> {
        Some(match self {
            Self::None => "none",
            Self::Volume => "volume",
            Self::Brightness => "brightness",
            Self::Scroll => "scroll",
            Self::Zoom => "zoom",
            Self::Seek(SeekPreset::Youtube) => "seek_youtube",
            Self::Seek(SeekPreset::General) => "seek_general",
            Self::UndoRedo => "undo_redo",
            Self::Tab => "tab",
            Self::Custom(_) => return None,
        })
    }

    fn from_name(s: &str) -> Option<Self> {
        Some(match s {
            "none" => Self::None,
            "volume" => Self::Volume,
            "brightness" => Self::Brightness,
            "scroll" => Self::Scroll,
            "zoom" => Self::Zoom,
            "seek_youtube" => Self::Seek(SeekPreset::Youtube),
            "seek_general" => Self::Seek(SeekPreset::General),
            "undo_redo" => Self::UndoRedo,
            "tab" => Self::Tab,
            _ => return None,
        })
    }
}

impl Serialize for EdgeTarget {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match (self.name(), self) {
            (Some(n), _) => s.serialize_str(n),
            (None, Self::Custom(c)) => CustomWire { custom: c }.serialize(s),
            (None, _) => s.serialize_str("none"),
        }
    }
}

impl<'de> Deserialize<'de> for EdgeTarget {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(match EdgeWire::deserialize(d)? {
            EdgeWire::Custom { custom } => Self::Custom(custom),
            EdgeWire::Other(_) => {
                scan_note(|s| {
                    s.bad.push(
                        "端のスライドの値が名前でも表でもないため「なし」にしました".to_owned(),
                    );
                });
                Self::None
            }
            EdgeWire::Name(n) => match Self::from_name(&n) {
                Some(t) => t,
                None => {
                    if n == "brush_size" {
                        scan_note(|s| s.brush_size += 1);
                    } else {
                        scan_note(|s| s.unknown.push(n));
                    }
                    Self::None
                }
            },
        })
    }
}

/// Edge sliders. Off for new configs (the recognizer is not in yet).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Edges {
    pub enabled: bool,
    pub left: EdgeTarget,
    pub right: EdgeTarget,
    pub top: EdgeTarget,
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

/// Keys of `Profile::edge_overrides`.
pub const EDGE_KEYS: [&str; 3] = ["edge.left", "edge.right", "edge.top"];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Profile {
    /// Executable name the profile applies to (case-insensitive, exact base name).
    pub exe_name: String,
    /// Optional name shown in the settings UI instead of the exe name.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub display_name: String,
    /// False switches yubi off for the app.
    pub enabled: bool,
    /// Per-gesture actions that replace the global ones in this app. The key is a gesture id
    /// (`swipe.4.left`); an explicit `none` turns that gesture off here. Only the ids the engine
    /// resolves are kept: tap, swipe, pinch (and `press.*`, which the pressure work will resolve).
    /// `corner.*` and `hold.*` ids are dropped on load and rejected on import until they are.
    pub overrides: BTreeMap<String, Action>,
    /// Per-edge targets that replace the global ones (`edge.left`, `edge.right`, `edge.top`).
    pub edge_overrides: BTreeMap<String, EdgeTarget>,
    /// Touchpad cursor speed in this app; `None` uses the global one (`[cursor] speed`).
    #[serde(deserialize_with = "cursor_speed")]
    pub cursor_speed: Option<u8>,
}

/// Windows' touchpad cursor speed, which yubi can manage (see `os::touchpad`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Cursor {
    /// Speed yubi keeps while it runs; `None` leaves Windows' setting alone.
    #[serde(deserialize_with = "cursor_speed")]
    pub speed: Option<u8>,
}

impl Cursor {
    /// The lowest and highest speed Windows accepts.
    pub const RANGE: (u8, u8) = (
        crate::os::touchpad::SPEED_MIN,
        crate::os::touchpad::SPEED_MAX,
    );

    pub fn clamp_speed(v: u8) -> u8 {
        v.clamp(Self::RANGE.0, Self::RANGE.1)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Ring {
    pub enabled: bool,
    #[serde(deserialize_with = "ring_fingers")]
    pub fingers: u8,
    pub items: Vec<RingItem>,
}

impl Default for Ring {
    fn default() -> Self {
        Self {
            enabled: true,
            fingers: 5,
            items: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct RingItem {
    pub label: String,
    pub action: Action,
}

/// Thresholds and speeds the recognizer reads. One authority for every number a user may want to
/// tune; debounce and stale deadlines stay constants of the engine.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Sensitivity {
    /// Cursor pixels per mm of finger travel while dragging.
    pub drag_gain: f32,
    /// Window pixels per mm while moving a window.
    pub window_gain: f32,
    /// Window pixels per mm while resizing (the resize gesture comes later).
    pub resize_gain: f32,
    /// Travel before a drag begins (mm).
    pub drag_start_mm: f32,
    /// Travel before a window move begins (mm).
    pub window_start_mm: f32,
    /// Centroid travel that makes a multi-finger swipe (mm).
    pub swipe_mm: f32,
    /// A tap moves less than this (mm).
    pub tap_travel_mm: f32,
    /// A tap is over within this time (ms).
    pub tap_ms: u16,
    /// A long hold moves less than this (mm).
    pub hold_travel_mm: f32,
    /// How long a long hold (and the ring menu) waits (ms).
    pub hold_ms: u16,
    /// After a 3-finger tap, a new touch within this time may move a window (ms).
    pub window_gap_ms: u16,
    /// A pinch in ends at or below this fraction of the start spread.
    pub pinch_in: f32,
    /// A pinch out ends at or above this multiple of the start spread.
    pub pinch_out: f32,
    /// A flick is at least this fast (mm/s).
    pub flick_mm_s: f32,
    /// Width of the edge strip (mm).
    pub edge_strip_mm: f32,
    /// Travel along an edge that makes one step (mm).
    pub edge_detent_mm: f32,
}

impl Default for Sensitivity {
    fn default() -> Self {
        Self {
            drag_gain: 12.0,
            window_gain: 12.0,
            resize_gain: 12.0,
            drag_start_mm: 1.5,
            window_start_mm: 8.0,
            swipe_mm: 15.0,
            tap_travel_mm: 2.0,
            tap_ms: 200,
            hold_travel_mm: 3.0,
            hold_ms: 500,
            window_gap_ms: 300,
            pinch_in: 0.75,
            pinch_out: 1.30,
            flick_mm_s: 400.0,
            edge_strip_mm: 6.0,
            edge_detent_mm: 2.0,
        }
    }
}

/// Lenient reader: any number is accepted (the range check is [`Sensitivity::clamp`]'s job), and a
/// value of the wrong type falls back to the default with a note, so one bad entry never makes the
/// whole file unusable.
impl<'de> Deserialize<'de> for Sensitivity {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let value = toml::Value::deserialize(d)?;
        let mut out = Self::default();
        let toml::Value::Table(table) = value else {
            scan_note(|s| {
                s.bad
                    .push("感度の設定が表の形ではないため、既定値を使います".to_owned());
            });
            return Ok(out);
        };
        let read = |key: &str| -> Option<f64> {
            let v = table.get(key)?;
            let n = match v {
                toml::Value::Integer(i) => Some(*i as f64),
                toml::Value::Float(f) if f.is_finite() => Some(*f),
                _ => None,
            };
            if n.is_none() {
                let label = Sensitivity::label_of(key).unwrap_or(key);
                scan_note(|s| {
                    s.bad.push(format!(
                        "感度の「{label}」の値が数値ではないため、既定値を使います"
                    ));
                });
            }
            n
        };
        let f = |key: &str, slot: &mut f32| {
            if let Some(n) = read(key) {
                *slot = n as f32;
            }
        };
        let ms = |key: &str, slot: &mut u16| {
            if let Some(n) = read(key) {
                *slot = n.clamp(0.0, f64::from(u16::MAX)) as u16;
                if !(0.0..=f64::from(u16::MAX)).contains(&n)
                    && let Some(label) = Sensitivity::label_of(key)
                {
                    scan_note(|s| s.raw_nums.push((label, n)));
                }
            }
        };
        f("drag_gain", &mut out.drag_gain);
        f("window_gain", &mut out.window_gain);
        f("resize_gain", &mut out.resize_gain);
        f("drag_start_mm", &mut out.drag_start_mm);
        f("window_start_mm", &mut out.window_start_mm);
        f("swipe_mm", &mut out.swipe_mm);
        f("tap_travel_mm", &mut out.tap_travel_mm);
        ms("tap_ms", &mut out.tap_ms);
        f("hold_travel_mm", &mut out.hold_travel_mm);
        ms("hold_ms", &mut out.hold_ms);
        ms("window_gap_ms", &mut out.window_gap_ms);
        f("pinch_in", &mut out.pinch_in);
        f("pinch_out", &mut out.pinch_out);
        f("flick_mm_s", &mut out.flick_mm_s);
        f("edge_strip_mm", &mut out.edge_strip_mm);
        f("edge_detent_mm", &mut out.edge_detent_mm);
        Ok(out)
    }
}

/// Allowed range of one sensitivity value.
#[derive(Debug, Clone, Copy)]
pub struct SensRange {
    pub min: f32,
    pub max: f32,
}

impl Sensitivity {
    pub const DRAG_GAIN: SensRange = SensRange {
        min: 2.0,
        max: 40.0,
    };
    pub const DRAG_START: SensRange = SensRange { min: 0.8, max: 5.0 };
    pub const WINDOW_START: SensRange = SensRange {
        min: 3.0,
        max: 20.0,
    };
    pub const SWIPE: SensRange = SensRange {
        min: 8.0,
        max: 40.0,
    };
    pub const TAP_TRAVEL: SensRange = SensRange { min: 1.0, max: 4.0 };
    pub const TAP_MS: SensRange = SensRange {
        min: 120.0,
        max: 300.0,
    };
    pub const HOLD_TRAVEL: SensRange = SensRange { min: 1.5, max: 5.0 };
    pub const HOLD_MS: SensRange = SensRange {
        min: 300.0,
        max: 1500.0,
    };
    pub const WINDOW_GAP: SensRange = SensRange {
        min: 200.0,
        max: 500.0,
    };
    pub const PINCH_IN: SensRange = SensRange {
        min: 0.60,
        max: 0.85,
    };
    pub const PINCH_OUT: SensRange = SensRange {
        min: 1.18,
        max: 1.60,
    };
    pub const FLICK: SensRange = SensRange {
        min: 200.0,
        max: 1000.0,
    };
    pub const EDGE_STRIP: SensRange = SensRange {
        min: 4.0,
        max: 10.0,
    };
    pub const EDGE_DETENT: SensRange = SensRange { min: 1.0, max: 6.0 };

    /// Every value with its Japanese name, in a fixed order (used to report what was clamped).
    pub fn values(&self) -> [(&'static str, f32); 16] {
        [
            ("ドラッグの速さ", self.drag_gain),
            ("ウィンドウ移動の速さ", self.window_gain),
            ("リサイズの速さ", self.resize_gain),
            ("ドラッグ開始の距離", self.drag_start_mm),
            ("ウィンドウ移動の開始距離", self.window_start_mm),
            ("スワイプの距離", self.swipe_mm),
            ("タップの許容移動", self.tap_travel_mm),
            ("タップの長さ", f32::from(self.tap_ms)),
            ("長押しの許容移動", self.hold_travel_mm),
            ("長押しの時間", f32::from(self.hold_ms)),
            ("ウィンドウ移動までの間隔", f32::from(self.window_gap_ms)),
            ("ピンチインの比率", self.pinch_in),
            ("ピンチアウトの比率", self.pinch_out),
            ("フリックの速さ", self.flick_mm_s),
            ("端の幅", self.edge_strip_mm),
            ("端の刻み", self.edge_detent_mm),
        ]
    }

    /// The TOML key of each entry of [`values`](Self::values), in the same order.
    pub const FIELDS: [&'static str; 16] = [
        "drag_gain",
        "window_gain",
        "resize_gain",
        "drag_start_mm",
        "window_start_mm",
        "swipe_mm",
        "tap_travel_mm",
        "tap_ms",
        "hold_travel_mm",
        "hold_ms",
        "window_gap_ms",
        "pinch_in",
        "pinch_out",
        "flick_mm_s",
        "edge_strip_mm",
        "edge_detent_mm",
    ];

    /// The Japanese name of the value stored under TOML key `field`.
    pub fn label_of(field: &str) -> Option<&'static str> {
        let i = Self::FIELDS.iter().position(|f| *f == field)?;
        Some(Self::default().values()[i].0)
    }

    /// Replace non-finite values with the default and pull everything into its range.
    pub fn clamp(&mut self) {
        let d = Self::default();
        let f = |v: f32, def: f32, r: SensRange| finite_or(v, def).clamp(r.min, r.max);
        let ms = |v: u16, r: SensRange| f32::from(v).clamp(r.min, r.max) as u16;
        self.drag_gain = f(self.drag_gain, d.drag_gain, Self::DRAG_GAIN);
        self.window_gain = f(self.window_gain, d.window_gain, Self::DRAG_GAIN);
        self.resize_gain = f(self.resize_gain, d.resize_gain, Self::DRAG_GAIN);
        self.drag_start_mm = f(self.drag_start_mm, d.drag_start_mm, Self::DRAG_START);
        self.window_start_mm = f(self.window_start_mm, d.window_start_mm, Self::WINDOW_START);
        self.swipe_mm = f(self.swipe_mm, d.swipe_mm, Self::SWIPE);
        self.tap_travel_mm = f(self.tap_travel_mm, d.tap_travel_mm, Self::TAP_TRAVEL);
        self.tap_ms = ms(self.tap_ms, Self::TAP_MS);
        self.hold_travel_mm = f(self.hold_travel_mm, d.hold_travel_mm, Self::HOLD_TRAVEL);
        self.hold_ms = ms(self.hold_ms, Self::HOLD_MS);
        self.window_gap_ms = ms(self.window_gap_ms, Self::WINDOW_GAP);
        self.pinch_in = f(self.pinch_in, d.pinch_in, Self::PINCH_IN);
        self.pinch_out = f(self.pinch_out, d.pinch_out, Self::PINCH_OUT);
        self.flick_mm_s = f(self.flick_mm_s, d.flick_mm_s, Self::FLICK);
        self.edge_strip_mm = f(self.edge_strip_mm, d.edge_strip_mm, Self::EDGE_STRIP);
        self.edge_detent_mm = f(self.edge_detent_mm, d.edge_detent_mm, Self::EDGE_DETENT);
    }
}

/// Look up the action field behind a gesture id token. `$m` is `mut` or empty, so the same table
/// serves the shared and the mutable accessor.
macro_rules! slot_of {
    ($cfg:expr, $id:expr, $($m:tt)*) => {
        match $id {
            "tap.4" => Some(&$($m)* $cfg.four.tap),
            "swipe.4.left" => Some(&$($m)* $cfg.four.swipe_left),
            "swipe.4.right" => Some(&$($m)* $cfg.four.swipe_right),
            "swipe.4.up" => Some(&$($m)* $cfg.four.swipe_up),
            "swipe.4.down" => Some(&$($m)* $cfg.four.swipe_down),
            "pinch.4.in" => Some(&$($m)* $cfg.four.pinch_in),
            "pinch.4.out" => Some(&$($m)* $cfg.four.pinch_out),
            "swipe.5.up" => Some(&$($m)* $cfg.five.swipe_up),
            "swipe.5.down" => Some(&$($m)* $cfg.five.swipe_down),
            "pinch.5.in" => Some(&$($m)* $cfg.five.pinch_in),
            "pinch.5.out" => Some(&$($m)* $cfg.five.pinch_out),
            "hold.long" => Some(&$($m)* $cfg.long_hold.action),
            other => {
                if let Some((n, deep)) = parse_press(other) {
                    match $cfg.press.fingers.len() > n - 1 {
                        true => {
                            let f = &$($m)* $cfg.press.fingers[n - 1];
                            Some(if deep { &$($m)* f.deep } else { &$($m)* f.light })
                        }
                        false => None,
                    }
                } else if let Some((i, swipe)) = parse_corner(other) {
                    match $cfg.corner.corners.len() > i {
                        true => {
                            let c = &$($m)* $cfg.corner.corners[i];
                            Some(if swipe { &$($m)* c.swipe } else { &$($m)* c.press })
                        }
                        false => None,
                    }
                } else {
                    None
                }
            }
        }
    };
}

/// `press.3.deep` -> (3, true).
fn parse_press(id: &str) -> Option<(usize, bool)> {
    let (n, kind) = id.strip_prefix("press.")?.split_once('.')?;
    let n: usize = n.parse().ok().filter(|n| (1..=5).contains(n))?;
    match kind {
        "light" => Some((n, false)),
        "deep" => Some((n, true)),
        _ => None,
    }
}

/// `corner.br.swipe` -> (3, true).
fn parse_corner(id: &str) -> Option<(usize, bool)> {
    let (corner, kind) = id.strip_prefix("corner.")?.split_once('.')?;
    let i = ["tl", "tr", "bl", "br"].iter().position(|c| *c == corner)?;
    match kind {
        "tap" => Some((i, false)),
        "swipe" => Some((i, true)),
        _ => None,
    }
}

/// `hold.thumb.index.swipe` -> (thumb hold, finger, trigger).
fn parse_hold(id: &str) -> Option<(bool, Finger, HoldTrigger)> {
    let mut parts = id.strip_prefix("hold.")?.split('.');
    let thumb = match parts.next()? {
        "finger" => false,
        "thumb" => true,
        _ => return None,
    };
    let finger_tok = parts.next()?;
    let finger = Finger::ALL.into_iter().find(|f| f.token() == finger_tok)?;
    let trigger_tok = parts.next()?;
    let trigger = HoldTrigger::ALL
        .into_iter()
        .find(|t| t.token() == trigger_tok)?;
    parts.next().is_none().then_some((thumb, finger, trigger))
}

fn keys_action(keys: &str) -> Action {
    if keys.trim().is_empty() {
        Action::None
    } else {
        Action::Keys(keys.to_owned())
    }
}

fn clamp_edge(e: &mut EdgeTarget) {
    if let EdgeTarget::Custom(c) = e {
        let d = Sensitivity::default().edge_detent_mm;
        c.step_mm = finite_or(c.step_mm, d).clamp(1.0, 6.0);
    }
}

impl Config {
    /// Clamp values into their valid ranges (deep threshold stays above light).
    pub fn normalize(&mut self) {
        self.drag.fingers = self.drag.fingers.clamp(3, 5);
        self.drag.lift_timeout_ms = self.drag.lift_timeout_ms.min(1000);
        self.long_hold.fingers = self.long_hold.fingers.clamp(1, 5);
        self.ring.fingers = self.ring.fingers.clamp(3, 5);
        self.sensitivity.clamp();
        let have = self.press.fingers.len();
        self.press
            .fingers
            .extend((have + 1..=5).map(PressFinger::for_count));
        self.press.fingers.truncate(5);
        self.corner.corners.resize_with(4, CornerCfg::default);
        for e in [
            &mut self.edge.left,
            &mut self.edge.right,
            &mut self.edge.top,
        ] {
            clamp_edge(e);
        }
        for p in &mut self.profiles {
            p.edge_overrides.values_mut().for_each(clamp_edge);
            p.cursor_speed = p.cursor_speed.map(Cursor::clamp_speed);
        }
        self.cursor.speed = self.cursor.speed.map(Cursor::clamp_speed);
        let p = &mut self.press;
        (p.light_threshold, p.deep_threshold) =
            clamp_pair(p.light_threshold, p.deep_threshold, DEFAULT_ONE);
        let (gl, gd) = (p.light_threshold, p.deep_threshold);
        for (i, f) in p.fingers.iter_mut().enumerate() {
            if i == 0 || (f.light_threshold.is_none() && f.deep_threshold.is_none()) {
                f.light_threshold = None;
                f.deep_threshold = None;
                continue;
            }
            let (l, d) = clamp_pair(
                f.light_threshold.unwrap_or(gl),
                f.deep_threshold.unwrap_or(gd),
                DEFAULT_MULTI,
            );
            f.light_threshold = Some(l);
            f.deep_threshold = Some(d);
        }
        // Drop calibrations that cannot be used (hand-edited or corrupt); keep the rest at 0.01.
        self.calibration.retain(|_, c| {
            c.baseline = q01(c.baseline);
            c.f1 = q01(c.f1);
            c.f2 = c.f2.map(q01);
            c.f3 = c.f3.map(q01);
            c.f4 = c.f4.map(q01);
            c.f5 = c.f5.map(q01);
            span_ok(c.baseline, c.f1)
        });
        for c in self.calibration.values_mut() {
            if c.f2.is_some_and(|f| !span_ok(c.baseline, f)) {
                c.f2 = None;
            }
            if c.f3.is_some_and(|f| !span_ok(c.baseline, f)) {
                c.f3 = None;
            }
            if c.f4.is_some_and(|f| !span_ok(c.baseline, f)) {
                c.f4 = None;
            }
            if c.f5.is_some_and(|f| !span_ok(c.baseline, f)) {
                c.f5 = None;
            }
        }
    }

    /// The global action bound to `id`. `None` when no config field backs the id (for example
    /// `tap.3`, which the recognizer has no binding for); `Some(Action::None)` for a bindable but
    /// unset gesture. Hold combos are keys only.
    pub fn binding(&self, id: &GestureId) -> Option<Action> {
        if let Some(a) = slot_of!(self, id.as_str(),) {
            return Some(a.clone());
        }
        let (thumb, finger, trigger) = parse_hold(id.as_str())?;
        let set = if thumb {
            &self.thumb_hold
        } else {
            &self.finger_hold
        };
        let keys = set
            .combos
            .iter()
            .find(|c| c.hold == finger && c.trigger == trigger)
            .map(|c| c.keys.as_str())
            .unwrap_or_default();
        Some(keys_action(keys))
    }

    /// Set the global action of `id`. False when no field backs the id, or when a hold combo is
    /// given something other than keys or `None`.
    #[allow(dead_code)] // used by the edge recognizer that comes later, and by tests
    pub fn set_binding(&mut self, id: &GestureId, action: Action) -> bool {
        if let Some(slot) = slot_of!(self, id.as_str(), mut) {
            *slot = action;
            return true;
        }
        let Some((thumb, finger, trigger)) = parse_hold(id.as_str()) else {
            return false;
        };
        let keys = match action {
            Action::Keys(k) => k,
            Action::None => String::new(),
            _ => return false,
        };
        let set = if thumb {
            &mut self.thumb_hold
        } else {
            &mut self.finger_hold
        };
        match set
            .combos
            .iter_mut()
            .find(|c| c.hold == finger && c.trigger == trigger)
        {
            Some(c) => c.keys = keys,
            None if keys.is_empty() => {}
            None => set.combos.push(Combo {
                hold: finger,
                trigger,
                keys,
            }),
        }
        true
    }

    /// The per-feature enable flags, in navigation order.
    pub fn feature_flags(&self) -> [bool; FEATURE_COUNT] {
        [
            self.drag.enabled,
            self.four.enabled,
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
        if self.four.enabled {
            let f = &self.four;
            for (name, a) in [
                ("4本指でタップ", &f.tap),
                ("4本指で左にスワイプ", &f.swipe_left),
                ("4本指で右にスワイプ", &f.swipe_right),
                ("4本指で上にスワイプ", &f.swipe_up),
                ("4本指で下にスワイプ", &f.swipe_down),
                ("4本指でピンチイン", &f.pinch_in),
                ("4本指でピンチアウト", &f.pinch_out),
            ] {
                if named(a) {
                    out.push((name.into(), a.label()));
                }
            }
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
                ("左端", &self.edge.left),
                ("右端", &self.edge.right),
                ("上端", &self.edge.top),
            ] {
                if e.kind() != EdgeKind::None {
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

/// Differences between the parsed and the normalized config, as Japanese notes.
///
/// `raw_nums` holds numbers from the file that the parsed `raw` could not keep (a `u16`
/// saturates): those are shown as written.
fn clamp_report(
    raw: &Config,
    norm: &Config,
    raw_nums: &[(&'static str, f64)],
    raw_speeds: &[(u8, i64)],
) -> Vec<String> {
    let mut w = Vec::new();
    let mut raw_speeds = raw_speeds.to_vec();
    let mut written = |v: u8| {
        raw_speeds
            .iter()
            .position(|(s, _)| *s == v)
            .map_or_else(|| v.to_string(), |i| raw_speeds.remove(i).1.to_string())
    };
    let mut note = |name: &str, before: String, after: String| {
        if before != after {
            w.push(format!(
                "{name} の値 {before} は範囲外のため {after} に補正しました"
            ));
        }
    };
    note(
        "ドラッグの指の本数",
        raw.drag.fingers.to_string(),
        norm.drag.fingers.to_string(),
    );
    note(
        "ドラッグの持ち上げ猶予 (ms)",
        raw.drag.lift_timeout_ms.to_string(),
        norm.drag.lift_timeout_ms.to_string(),
    );
    note(
        "長押しの指の本数",
        raw.long_hold.fingers.to_string(),
        norm.long_hold.fingers.to_string(),
    );
    note(
        "リングメニューの指の本数",
        raw.ring.fingers.to_string(),
        norm.ring.fingers.to_string(),
    );
    for ((name, before), (_, after)) in raw
        .sensitivity
        .values()
        .into_iter()
        .zip(norm.sensitivity.values())
    {
        let before = raw_nums
            .iter()
            .find(|(n, _)| *n == name)
            .map_or_else(|| before.to_string(), |(_, v)| v.to_string());
        note(name, before, after.to_string());
    }
    note(
        "軽い押し込みのしきい値",
        format!("{}", raw.press.light_threshold),
        format!("{}", norm.press.light_threshold),
    );
    note(
        "深い押し込みのしきい値",
        format!("{}", raw.press.deep_threshold),
        format!("{}", norm.press.deep_threshold),
    );
    let step = |e: &EdgeTarget| match e {
        EdgeTarget::Custom(c) => Some(c.step_mm),
        _ => None,
    };
    for (name, r, n) in [
        ("左端のスライドの刻み (mm)", &raw.edge.left, &norm.edge.left),
        (
            "右端のスライドの刻み (mm)",
            &raw.edge.right,
            &norm.edge.right,
        ),
        ("上端のスライドの刻み (mm)", &raw.edge.top, &norm.edge.top),
    ] {
        if let (Some(b), Some(a)) = (step(r), step(n)) {
            note(name, b.to_string(), a.to_string());
        }
    }
    for (rp, np) in raw.profiles.iter().zip(&norm.profiles) {
        for (key, r) in &rp.edge_overrides {
            if let (Some(b), Some(a)) = (step(r), np.edge_overrides.get(key).and_then(step)) {
                note(
                    &format!("「{}」の {key} の刻み (mm)", rp.exe_name.trim()),
                    b.to_string(),
                    a.to_string(),
                );
            }
        }
    }
    if let (Some(b), Some(a)) = (raw.cursor.speed, norm.cursor.speed) {
        note("カーソルの速度", written(b), a.to_string());
    }
    for (rp, np) in raw.profiles.iter().zip(&norm.profiles) {
        if let (Some(b), Some(a)) = (rp.cursor_speed, np.cursor_speed) {
            note(
                &format!("「{}」のカーソルの速度", rp.exe_name.trim()),
                written(b),
                a.to_string(),
            );
        }
    }
    if raw.press.fingers.len() != norm.press.fingers.len() {
        w.push(format!(
            "押し込みの指の設定が {} 個だったため {} 個に補正しました",
            raw.press.fingers.len(),
            norm.press.fingers.len()
        ));
    }
    if raw.corner.corners.len() != norm.corner.corners.len() {
        w.push(format!(
            "コーナーの設定が {} 個だったため {} 個に補正しました",
            raw.corner.corners.len(),
            norm.corner.corners.len()
        ));
    }
    w
}

/// `%APPDATA%\yubi\config.toml` (falls back to the current directory).
pub fn default_path() -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .or_else(|| std::env::var_os("XDG_CONFIG_HOME"))
        .map(PathBuf::from)
        .unwrap_or_default();
    base.join("yubi").join("config.toml")
}

/// Why a config text could not be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// Not valid TOML, or not a yubi config (message from the parser).
    Syntax(String),
    /// Written by a newer yubi (`schema_version` above [`SCHEMA_VERSION`]).
    FutureVersion(u32),
}

/// A parsed, migrated and normalized config text.
#[derive(Debug, Clone, PartialEq)]
pub struct Parsed {
    pub config: Config,
    /// Japanese notes: migrations, dropped entries, clamped values.
    pub warnings: Vec<String>,
    /// Action or edge tokens this build does not know (loaded as `None`).
    pub unknown_tokens: Vec<String>,
    /// Executable names that appear in more than one profile (the first one is kept).
    pub duplicate_profiles: Vec<String>,
    /// Profile override keys no config field backs (`exe: id`); they are dropped.
    pub unbindable_overrides: Vec<String>,
    /// Profile override ids (`exe: id`) of gestures the engine does not resolve yet
    /// (`corner.*`, `hold.*`); they are dropped.
    pub inert_overrides: Vec<String>,
    /// Profile override keys that are no gesture id or edge key at all (`exe: key`); dropped.
    pub bad_override_keys: Vec<String>,
    /// The text was an older schema, or held entries that were rewritten: save it back.
    pub rewrite: bool,
}

/// Move the v1 layout to v2 inside the TOML value, before it is read into [`Config`]:
/// the two hold times become `sensitivity.hold_ms`, ring commands become typed actions.
fn migrate_v1(root: &mut toml::Table, notes: &mut Vec<String>) {
    use toml::Value;
    let mut take_hold = |table: &str| {
        root.get_mut(table)
            .and_then(Value::as_table_mut)
            .and_then(|t| t.remove("hold_ms"))
    };
    let hold = take_hold("long_hold");
    let ring_hold = take_hold("ring");
    if let Some(h) = hold.or(ring_hold) {
        let sens = root
            .entry("sensitivity")
            .or_insert_with(|| Value::Table(toml::Table::new()));
        if let Some(t) = sens.as_table_mut() {
            t.entry("hold_ms").or_insert(h);
        }
    }
    if let Some(items) = root
        .get_mut("ring")
        .and_then(Value::as_table_mut)
        .and_then(|r| r.get_mut("items"))
        .and_then(Value::as_array_mut)
    {
        let old = std::mem::take(items);
        for item in old {
            let Value::Table(mut t) = item else {
                notes.push("リングメニューの項目が表の形ではないため削除しました".to_owned());
                continue;
            };
            let command = t.remove("command");
            if !t.contains_key("action") {
                let label = t
                    .get("label")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                if command.as_ref().is_some_and(|c| !c.is_str()) {
                    notes.push(format!(
                        "リングメニューの「{label}」のコマンドが文字列ではないため削除しました"
                    ));
                    continue;
                }
                let cmd = command
                    .as_ref()
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .trim()
                    .to_owned();
                if cmd.is_empty() {
                    t.insert("action".into(), Value::String("none".into()));
                } else if cmd.parse::<crate::os::keys::KeyCombo>().is_ok() {
                    t.insert("action".into(), Value::String(format!("keys:{cmd}")));
                } else {
                    notes.push(format!(
                        "リングメニューの「{label}」のコマンド「{cmd}」はキー入力として読めないため削除しました"
                    ));
                    continue;
                }
            }
            items.push(Value::Table(t));
        }
    }
    root.insert("schema_version".into(), Value::Integer(2));
}

/// Parse one config text: read the schema version, migrate older layouts, deserialize, report
/// unknown and legacy entries, drop duplicate profiles and bad override keys, then normalize.
pub fn parse_text(text: &str) -> Result<Parsed, ParseError> {
    let mut root: toml::Table =
        toml::from_str(text).map_err(|e| ParseError::Syntax(e.to_string()))?;
    let version = match root.get("schema_version") {
        None => 1,
        Some(v) => v
            .as_integer()
            .ok_or_else(|| ParseError::Syntax("schema_version は整数で指定してください".into()))?
            .max(1),
    };
    if version > i64::from(SCHEMA_VERSION) {
        return Err(ParseError::FutureVersion(
            u32::try_from(version).unwrap_or(u32::MAX),
        ));
    }
    let mut warnings = Vec::new();
    let migrated = version < i64::from(SCHEMA_VERSION);
    if migrated {
        migrate_v1(&mut root, &mut warnings);
    }
    scan_note(|s| *s = Scan::default());
    let result = toml::Value::Table(root).try_into::<Config>();
    let scan = SCAN.with(|s| std::mem::take(&mut *s.borrow_mut()));
    let mut config = result.map_err(|e| ParseError::Syntax(e.to_string()))?;
    config.schema_version = SCHEMA_VERSION;

    if !scan.legacy.is_empty() {
        warnings.push(format!(
            "以前の版の「{}」は音量・明るさの変化量が決まらないため「なし」にしました。割り当てを選び直してください",
            scan.legacy.join("」「")
        ));
    }
    if scan.brush_size > 0 {
        warnings
            .push("端のスライドの「ブラシサイズ」は廃止されたため「なし」にしました".to_owned());
    }
    for t in &scan.unknown {
        warnings.push(format!("未対応の値「{t}」は「なし」として読み込みました"));
    }
    warnings.extend(scan.bad.iter().cloned());

    let mut seen = Vec::new();
    let mut duplicate_profiles = Vec::new();
    config.profiles.retain(|p| {
        // The key the resolver matches on: the lowercased basename.
        let key = crate::engine::resolve::basename(&p.exe_name).to_ascii_lowercase();
        if key.is_empty() {
            return true;
        }
        if seen.contains(&key) {
            duplicate_profiles.push(p.exe_name.trim().to_owned());
            false
        } else {
            seen.push(key);
            true
        }
    });
    let mut bad_override_keys = Vec::new();
    let mut dropped_keys = Vec::new();
    for p in &mut config.profiles {
        let before = p.overrides.len() + p.edge_overrides.len();
        let exe = p.exe_name.trim().to_owned();
        p.overrides.retain(|k, _| {
            let ok = GestureId::parse(k).is_some();
            if !ok {
                bad_override_keys.push(format!("{exe}: {k}"));
            }
            ok
        });
        p.edge_overrides.retain(|k, _| {
            let ok = EDGE_KEYS.contains(&k.as_str());
            if !ok {
                bad_override_keys.push(format!("{exe}: {k}"));
            }
            ok
        });
        dropped_keys.push(before - (p.overrides.len() + p.edge_overrides.len()));
    }

    let raw = config.clone();
    config.normalize();
    // Overrides for ids that have a name but no global field (`tap.5`, `swipe.3.left` ...) could
    // never be resolved: drop them. Checked after normalizing so `press.N` / corner slots exist.
    let mut unbindable_overrides = Vec::new();
    let mut inert_overrides = Vec::new();
    let mut profiles = std::mem::take(&mut config.profiles);
    for (p, dropped) in profiles.iter_mut().zip(dropped_keys) {
        let exe = p.exe_name.trim().to_owned();
        let before = p.overrides.len();
        let mut inert = 0usize;
        p.overrides.retain(|k, _| {
            // The engine resolves no corner or hold gesture yet: an override would do nothing.
            if k.starts_with("corner.") || k.starts_with("hold.") {
                inert_overrides.push(format!("{exe}: {k}"));
                inert += 1;
                return false;
            }
            let ok = GestureId::parse(k).is_some_and(|id| config.binding(&id).is_some());
            if !ok {
                unbindable_overrides.push(format!("{exe}: {k}"));
            }
            ok
        });
        if dropped > 0 || p.overrides.len() + inert != before {
            warnings.push(format!(
                "「{exe}」のアプリ別設定にある未対応のジェスチャー名を削除しました"
            ));
        }
        if inert > 0 {
            warnings.push(format!(
                "「{exe}」のアプリ別設定にあるコーナー・ホールドの項目は、まだ適用されないため削除しました"
            ));
        }
    }
    config.profiles = profiles;
    warnings.extend(clamp_report(
        &raw,
        &config,
        &scan.raw_nums,
        &scan.raw_speeds,
    ));
    Ok(Parsed {
        config,
        warnings,
        unknown_tokens: scan.unknown,
        duplicate_profiles,
        unbindable_overrides,
        inert_overrides,
        bad_override_keys,
        rewrite: migrated || !scan.legacy.is_empty() || scan.brush_size > 0,
    })
}

/// A sibling of `path` named `config.toml.<tag>-<unix-seconds>` that does not exist yet (a
/// `-<n>` suffix keeps an older one from ever being overwritten).
fn unique_sibling(path: &Path, tag: &str) -> PathBuf {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let mut n = 0u32;
    loop {
        let ext = if n == 0 {
            format!("toml.{tag}-{secs}")
        } else {
            format!("toml.{tag}-{secs}-{n}")
        };
        let candidate = path.with_extension(ext);
        if !candidate.exists() {
            return candidate;
        }
        n += 1;
    }
}

/// Move an unusable config file to a fresh `config.toml.bad-<unix-seconds>` name (never overwriting
/// an older backup) so the next autosave cannot destroy the user's edits.
fn move_aside(path: &Path) {
    let target = unique_sibling(path, "bad");
    match fs::rename(path, &target) {
        Ok(()) => eprintln!("yubi: moved the unusable config to {}", target.display()),
        Err(e) => eprintln!("yubi: cannot move {} aside: {e}", path.display()),
    }
}

/// What loading the config file produced.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Loaded {
    pub config: Config,
    /// Japanese notes for the user (migrations, dropped entries, a file that was set aside).
    pub warnings: Vec<String>,
    /// The file was written by a newer yubi: it is left untouched and must not be autosaved over.
    pub future_version: Option<u32>,
    /// The file on disk is still the old layout and its backup copy failed: the first save must
    /// retry the copy ([`backup_before_first_save`]) and must not happen while that fails.
    pub needs_backup: bool,
}

/// Why [`save_migrated`] did not finish.
struct MigrateFailure {
    /// A Japanese note for the user.
    note: String,
    /// The old file is still on disk and has no backup yet.
    backup_missing: bool,
}

/// Copy the original to `backup`, then save the migrated config over it. Without the copy nothing
/// is saved. The error is a Japanese note for the user.
fn save_migrated(path: &Path, cfg: &Config, backup: &Path) -> Result<(), MigrateFailure> {
    fs::copy(path, backup).map_err(|e| MigrateFailure {
        note: format!(
            "移行前の設定を控えられなかったため、移行後の設定はまだ保存していません。控えを作れるまで、設定を変更しても保存しません（この起動中だけ移行した設定を使います）: {e}"
        ),
        backup_missing: true,
    })?;
    save(path, cfg).map_err(|e| MigrateFailure {
        note: format!(
            "移行した設定を保存できませんでした。元のファイルはそのまま残っています: {e}"
        ),
        backup_missing: false,
    })
}

/// The gate in front of the first save of a session that loaded an old file whose backup failed:
/// retry the copy, and clear `needs_backup` once it worked. On `Err` the caller must not save
/// (that would overwrite the old file with nothing kept). The error is a Japanese note.
pub fn backup_before_first_save(path: &Path, needs_backup: &mut bool) -> Result<(), String> {
    if !*needs_backup {
        return Ok(());
    }
    if !path.exists() {
        // The old file is gone: there is nothing left to protect.
        *needs_backup = false;
        return Ok(());
    }
    match fs::copy(path, unique_sibling(path, "v1")) {
        Ok(_) => {
            *needs_backup = false;
            Ok(())
        }
        Err(e) => Err(format!(
            "移行前の設定の控えを作れないため、設定の変更を保存していません。控えを作れるようになるまで保存しません: {e}"
        )),
    }
}

/// Load the config; a missing file yields defaults. A file that cannot be read or parsed is moved
/// aside (see `move_aside`) and defaults are used. A schema older than this build is migrated and
/// saved back (the old file is kept as `config.toml.v1-<unix-seconds>`); a newer one is left alone.
pub fn load_report(path: &Path) -> Loaded {
    let text = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Loaded::default(),
        Err(e) => {
            eprintln!("yubi: cannot read config {}: {e}", path.display());
            move_aside(path);
            return Loaded {
                warnings: vec![format!(
                    "設定ファイルを読めなかったため、別名で退避して既定値で起動しました: {e}"
                )],
                ..Loaded::default()
            };
        }
    };
    match parse_text(&text) {
        Ok(p) => {
            let mut warnings = p.warnings;
            for d in &p.duplicate_profiles {
                warnings.push(format!(
                    "アプリ別設定「{d}」が重複しています。最初の設定だけを使います"
                ));
            }
            let mut needs_backup = false;
            if p.rewrite {
                // Keep the original first; without that copy the migrated file is not saved
                // (the migrated config still runs, in memory).
                if let Err(f) = save_migrated(path, &p.config, &unique_sibling(path, "v1")) {
                    eprintln!("yubi: {}", f.note);
                    needs_backup = f.backup_missing;
                    warnings.push(f.note);
                }
            }
            Loaded {
                config: p.config,
                warnings,
                future_version: None,
                needs_backup,
            }
        }
        Err(ParseError::Syntax(e)) => {
            eprintln!("yubi: invalid config {}: {e}", path.display());
            move_aside(path);
            Loaded {
                warnings: vec![format!(
                    "設定ファイルを読み取れなかったため、別名で退避して既定値で起動しました: {e}"
                )],
                ..Loaded::default()
            }
        }
        Err(ParseError::FutureVersion(v)) => Loaded {
            warnings: vec![format!(
                "設定ファイルは新しい版 (schema_version = {v}) で保存されているため読み込めません。ファイルには手を加えず、既定値で動作します。この間の変更は保存されません"
            )],
            future_version: Some(v),
            ..Loaded::default()
        },
    }
}

/// [`load_report`] without the notes.
#[allow(dead_code, reason = "tests and tools load without the report")]
pub fn load(path: &Path) -> Config {
    load_report(path).config
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

    fn rich() -> Config {
        let mut cfg = Config {
            gestures_enabled: false,
            ..Config::default()
        };
        cfg.general.theme = ThemeChoice::Light;
        cfg.drag.enabled = true;
        cfg.drag.fingers = 4;
        cfg.four.swipe_left = Action::Keys("Ctrl+Alt+Right".into());
        cfg.four.tap = Action::VolumeStep(-4);
        cfg.four.pinch_in = Action::ToggleMute;
        cfg.five.pinch_in = Action::TaskView;
        cfg.five.swipe_up = Action::Keys("Win+Tab".into());
        cfg.five.swipe_down = Action::BrightnessStep(5);
        cfg.press.haptic_strength = HapticStrength::Firm;
        cfg.press.fingers[2].deep = Action::RingMenu;
        cfg.long_hold.action = Action::MiddleClick;
        cfg.finger_hold.combos.push(Combo {
            hold: Finger::Middle,
            trigger: HoldTrigger::Swipe,
            keys: "Alt+Tab".into(),
        });
        cfg.corner.corners[3].swipe = Action::Find;
        cfg.edge.enabled = true;
        cfg.edge.left = EdgeTarget::Custom(CustomEdge {
            forward: "Ctrl+Tab".into(),
            back: "Ctrl+Shift+Tab".into(),
            step_mm: 3.5,
        });
        cfg.edge.right = EdgeTarget::Volume;
        cfg.edge.top = EdgeTarget::Seek(SeekPreset::Youtube);
        cfg.window.flick_snap = false;
        cfg.sensitivity.swipe_mm = 22.0;
        cfg.sensitivity.hold_ms = 800;
        let mut p = Profile {
            exe_name: "code.exe".into(),
            enabled: true,
            ..Profile::default()
        };
        p.overrides
            .insert("swipe.4.left".into(), Action::Keys("Ctrl+PageUp".into()));
        p.overrides.insert("tap.4".into(), Action::None);
        p.edge_overrides
            .insert("edge.left".into(), EdgeTarget::Scroll);
        cfg.profiles.push(p);
        cfg.ring.items.push(RingItem {
            label: "コピー".into(),
            action: Action::Keys("ctrl+c".into()),
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
    fn default_is_schema_2_and_round_trips_through_text() {
        let cfg = Config::default();
        assert_eq!(cfg.schema_version, SCHEMA_VERSION);
        let text = toml::to_string_pretty(&cfg).unwrap();
        assert!(text.contains("schema_version = 2"), "{text}");
        let parsed = parse_text(&text).unwrap();
        assert_eq!(parsed.config, cfg);
        assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
        assert!(!parsed.rewrite);
    }

    #[test]
    fn missing_file_gives_defaults() {
        assert_eq!(
            load(&temp_path("missing").join("none.toml")),
            Config::default()
        );
    }

    /// A schema 1 file with every table the old versions wrote.
    const V1_FIXTURE: &str = r#"
gestures_enabled = false

[general]
theme = "light"
start_with_windows = true

[gestures]
three_finger_drag = true
drag_fingers = 4
lift_timeout_ms = 250
keep_drag_on_release = true

[four]
enabled = true
tap = "volume"
swipe_left = "keys:Ctrl+Alt+Right"
swipe_right = "desktop_prev"
swipe_up = "task_view"
swipe_down = "brightness"
pinch_in = "middle_click"
pinch_out = "find"

[five]
enabled = true
pinch_in = "show_desktop"
swipe_up = "app_switch"

[press]
enabled = true
light_threshold = 0.1
deep_threshold = 0.16
haptic_strength = "firm"

[[press.fingers]]
light = "right_click"
deep = "ring_menu"

[long_hold]
enabled = true
fingers = 3
hold_ms = 700
action = "tab_switch"

[finger_hold]
enabled = true
combos = [{ hold = "middle", trigger = "swipe", keys = "Alt+Tab" }]

[thumb_hold]
enabled = true
combos = [{ hold = "thumb", trigger = "tap", keys = "Ctrl+Z" }]

[corner]
enabled = true

[[corner.corners]]
press = "find"

[edge]
enabled = true
left = "brush_size"
right = "volume"
top = "brightness"

[window]
enabled = true
flick_snap = false

[[profiles]]
exe_name = "code.exe"
enabled = true

[ring]
enabled = true
fingers = 4
hold_ms = 900
items = [
  { label = "コピー", command = "ctrl+c" },
  { label = "空", command = "" },
  { label = "危険", command = "rm -rf /" },
]
"#;

    #[test]
    fn v1_file_migrates_to_v2() {
        let p = parse_text(V1_FIXTURE).unwrap();
        let c = &p.config;
        assert_eq!(c.schema_version, 2);
        assert!(p.rewrite);
        assert!(!c.gestures_enabled);
        assert_eq!(c.general.theme, ThemeChoice::Light);
        assert!(c.drag.enabled);
        assert_eq!(c.drag.fingers, 4);
        assert_eq!(c.drag.lift_timeout_ms, 250);
        assert!(c.drag.keep_drag_on_release);
        // Ambiguous legacy tokens are dropped and reported once.
        assert_eq!(c.four.tap, Action::None);
        assert_eq!(c.four.swipe_down, Action::None);
        assert_eq!(c.four.swipe_left, Action::Keys("Ctrl+Alt+Right".into()));
        assert_eq!(c.four.pinch_out, Action::Find);
        assert_eq!(c.five.pinch_in, Action::ShowDesktop);
        assert_eq!(c.five.swipe_up, Action::AppSwitch);
        assert_eq!(
            (c.press.light_threshold, c.press.deep_threshold),
            (0.1, 0.16)
        );
        assert_eq!(c.press.fingers.len(), 5);
        assert_eq!(c.press.fingers[0].deep, Action::RingMenu);
        assert_eq!(c.long_hold.action, Action::TabSwitch);
        assert_eq!(c.finger_hold.combos[0].keys, "Alt+Tab");
        assert_eq!(c.thumb_hold.combos[0].hold, Finger::Thumb);
        assert_eq!(c.corner.corners[0].press, Action::Find);
        assert_eq!(c.corner.corners.len(), 4);
        // brush_size is gone.
        assert_eq!(c.edge.left, EdgeTarget::None);
        assert_eq!(c.edge.right, EdgeTarget::Volume);
        assert_eq!(c.edge.top, EdgeTarget::Brightness);
        assert!(!c.window.flick_snap);
        assert_eq!(c.profiles[0].exe_name, "code.exe");
        assert!(c.profiles[0].overrides.is_empty());
        // long_hold.hold_ms wins over ring.hold_ms and lands in [sensitivity].
        assert_eq!(c.sensitivity.hold_ms, 700);
        assert_eq!(c.ring.fingers, 4);
        // Ring commands: a key combination becomes a typed action, empty becomes none, the rest is
        // dropped (strings are never shell commands).
        assert_eq!(c.ring.items.len(), 2);
        assert_eq!(c.ring.items[0].action, Action::Keys("ctrl+c".into()));
        assert_eq!(c.ring.items[1].action, Action::None);
        let all = p.warnings.join("\n");
        assert!(all.contains("「brightness」「volume」"), "{all}");
        assert!(all.contains("ブラシサイズ"), "{all}");
        assert!(all.contains("rm -rf /"), "{all}");
        assert!(p.unknown_tokens.is_empty());
    }

    #[test]
    fn ring_hold_is_used_when_long_hold_has_none() {
        let p = parse_text("[ring]\nhold_ms = 900\n").unwrap();
        assert_eq!(p.config.sensitivity.hold_ms, 900);
        // An explicit [sensitivity] value is not overwritten by the old field.
        let p = parse_text("[ring]\nhold_ms = 900\n[sensitivity]\nhold_ms = 400\n").unwrap();
        assert_eq!(p.config.sensitivity.hold_ms, 400);
    }

    #[test]
    fn load_saves_the_migrated_file_and_keeps_the_old_one() {
        let dir = temp_dir("migrate");
        let path = dir.join("config.toml");
        fs::write(&path, V1_FIXTURE).unwrap();
        let first = load_report(&path);
        assert_eq!(first.config.schema_version, 2);
        assert!(!first.warnings.is_empty());
        assert!(first.future_version.is_none());
        let saved = fs::read_to_string(&path).unwrap();
        assert!(saved.contains("schema_version = 2"));
        let backups: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("config.toml.v1-")
            })
            .collect();
        assert_eq!(backups.len(), 1);
        assert_eq!(fs::read_to_string(backups[0].path()).unwrap(), V1_FIXTURE);
        // The second load reads the saved v2 file: same config, nothing left to warn about.
        let second = load_report(&path);
        assert_eq!(second.config, first.config);
        assert!(second.warnings.is_empty(), "{:?}", second.warnings);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn future_version_is_left_untouched() {
        let text = "schema_version = 3\ngestures_enabled = false\n[mystery]\nx = 1\n";
        assert_eq!(parse_text(text), Err(ParseError::FutureVersion(3)));
        let dir = temp_dir("future");
        let path = dir.join("config.toml");
        fs::write(&path, text).unwrap();
        let got = load_report(&path);
        assert_eq!(got.future_version, Some(3));
        assert_eq!(got.config, Config::default());
        assert!(got.warnings[0].contains("schema_version = 3"));
        assert_eq!(fs::read_to_string(&path).unwrap(), text);
        assert!(bad_files(&dir).is_empty());
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn migration_is_not_saved_when_the_backup_copy_fails() {
        let dir = temp_dir("migrate-nobackup");
        let path = dir.join("config.toml");
        fs::write(&path, V1_FIXTURE).unwrap();
        let cfg = parse_text(V1_FIXTURE).unwrap().config;
        let missing = dir.join("no-such-dir").join("config.toml.v1-1");
        let err = save_migrated(&path, &cfg, &missing).unwrap_err();
        assert!(err.note.contains("控えられなかった"), "{}", err.note);
        assert!(err.note.contains("保存しません"), "{}", err.note);
        assert!(err.backup_missing);
        assert_eq!(fs::read_to_string(&path).unwrap(), V1_FIXTURE);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn first_save_is_gated_on_the_backup() {
        let dir = temp_dir("backup-gate");
        let path = dir.join("config.toml");
        // Nothing pending: always fine.
        let mut pending = false;
        assert!(backup_before_first_save(&path, &mut pending).is_ok());
        // The copy still fails (the "file" is a directory): no save, flag stays.
        fs::create_dir_all(&path).unwrap();
        let mut pending = true;
        let err = backup_before_first_save(&path, &mut pending).unwrap_err();
        assert!(err.contains("保存していません"), "{err}");
        assert!(pending);
        fs::remove_dir_all(&path).unwrap();
        // The copy works now: the old file is kept, the flag clears, a later save is allowed.
        fs::write(&path, V1_FIXTURE).unwrap();
        assert!(backup_before_first_save(&path, &mut pending).is_ok());
        assert!(!pending);
        let kept: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".v1-"))
            .collect();
        assert_eq!(kept.len(), 1);
        assert_eq!(fs::read_to_string(kept[0].path()).unwrap(), V1_FIXTURE);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn legacy_and_unknown_action_tokens_load_as_none() {
        let p = parse_text(
            "schema_version = 2\n[four]\ntap = \"volume\"\nswipe_up = \"fly\"\nswipe_down = \"brightness-5\"\n",
        )
        .unwrap();
        assert_eq!(p.config.four.tap, Action::None);
        assert_eq!(p.config.four.swipe_up, Action::None);
        assert_eq!(p.config.four.swipe_down, Action::BrightnessStep(-5));
        assert_eq!(p.unknown_tokens, vec!["fly".to_owned()]);
        assert!(p.warnings.iter().any(|w| w.contains("「volume」")));
        assert!(p.warnings.iter().any(|w| w.contains("「fly」")));
        assert!(p.rewrite);
    }

    #[test]
    fn typed_action_tokens() {
        assert_eq!(Action::VolumeStep(2).to_token(), "volume+2");
        assert_eq!(Action::VolumeStep(-10).to_token(), "volume-10");
        assert_eq!(Action::BrightnessStep(-5).to_token(), "brightness-5");
        assert_eq!(Action::ToggleMute.to_token(), "mute");
        assert_eq!(Action::from_token("volume+2"), Action::VolumeStep(2));
        assert_eq!(
            Action::from_token("brightness-5"),
            Action::BrightnessStep(-5)
        );
        assert_eq!(Action::from_token("mute"), Action::ToggleMute);
        assert_eq!(Action::parse_token("volume"), TokenParse::Legacy);
        assert_eq!(Action::parse_token("brightness"), TokenParse::Legacy);
        // The sign is required and the step is bounded.
        assert_eq!(Action::parse_token("volume2"), TokenParse::Unknown);
        assert_eq!(Action::parse_token("volume+99"), TokenParse::Unknown);
        assert_eq!(Action::parse_token("volume+"), TokenParse::Unknown);
        assert_eq!(Action::VolumeStep(3).label(), "音量 +3");
        for a in [
            Action::VolumeStep(7),
            Action::BrightnessStep(-50),
            Action::ToggleMute,
        ] {
            assert_eq!(Action::from_token(&a.to_token()), a);
        }
    }

    #[test]
    fn legacy_brush_size_edge_loads_as_none_and_round_trips() {
        let p = parse_text(
            "schema_version = 2\n[edge]\nenabled = true\nleft = \"brush_size\"\nright = \"volume\"\n",
        )
        .unwrap();
        assert_eq!(p.config.edge.left, EdgeTarget::None);
        assert_eq!(p.config.edge.right, EdgeTarget::Volume);
        assert!(p.warnings.iter().any(|w| w.contains("ブラシサイズ")));
        assert!(p.unknown_tokens.is_empty());
        // Saved back, it no longer mentions brush_size.
        let text = toml::to_string_pretty(&p.config).unwrap();
        assert!(!text.contains("brush_size"));
        assert_eq!(parse_text(&text).unwrap().config.edge, p.config.edge);
        let p = parse_text("schema_version = 2\n[edge]\ntop = \"warp\"\n").unwrap();
        assert_eq!(p.config.edge.top, EdgeTarget::None);
        assert_eq!(p.unknown_tokens, vec!["warp".to_owned()]);
    }

    #[test]
    fn edge_targets_round_trip_and_default_off() {
        let cfg = rich();
        let text = toml::to_string_pretty(&cfg).unwrap();
        let back = parse_text(&text).unwrap().config;
        assert_eq!(back.edge, cfg.edge);
        assert_eq!(back.profiles, cfg.profiles);
        let d = Edges::default();
        assert!(!d.enabled);
        assert_eq!(
            [&d.left, &d.right, &d.top],
            [&EdgeTarget::None, &EdgeTarget::None, &EdgeTarget::None]
        );
        for kind in EdgeKind::ALL {
            let mut e = EdgeTarget::None;
            e.set_kind(kind);
            assert_eq!(e.kind(), kind);
            assert!(!kind.label().is_empty());
        }
    }

    #[test]
    fn edge_step_mapping() {
        let k = |s: &str| vec![EdgeOut::Act(Action::Keys(s.into()))];
        assert!(EdgeTarget::None.step(true).is_empty());
        assert_eq!(
            EdgeTarget::Volume.step(true),
            vec![EdgeOut::Act(Action::VolumeStep(2))]
        );
        assert_eq!(
            EdgeTarget::Volume.step(false),
            vec![EdgeOut::Act(Action::VolumeStep(-2))]
        );
        assert_eq!(
            EdgeTarget::Brightness.step(false),
            vec![EdgeOut::Act(Action::BrightnessStep(-5))]
        );
        assert_eq!(EdgeTarget::Scroll.step(true), vec![EdgeOut::Wheel(-1)]);
        assert_eq!(EdgeTarget::Scroll.step(false), vec![EdgeOut::Wheel(1)]);
        assert_eq!(EdgeTarget::Zoom.step(true), vec![EdgeOut::CtrlWheel(-1)]);
        assert_eq!(EdgeTarget::Seek(SeekPreset::Youtube).step(true), k("L"));
        assert_eq!(EdgeTarget::Seek(SeekPreset::Youtube).step(false), k("J"));
        assert_eq!(EdgeTarget::Seek(SeekPreset::General).step(true), k("Right"));
        assert_eq!(EdgeTarget::Seek(SeekPreset::General).step(false), k("Left"));
        assert_eq!(EdgeTarget::UndoRedo.step(true), k("Ctrl+Y"));
        assert_eq!(EdgeTarget::UndoRedo.step(false), k("Ctrl+Z"));
        assert_eq!(EdgeTarget::Tab.step(true), k("Ctrl+Tab"));
        assert_eq!(EdgeTarget::Tab.step(false), k("Ctrl+Shift+Tab"));
        let custom = EdgeTarget::Custom(CustomEdge {
            forward: " Alt+Right ".into(),
            back: "not a key".into(),
            step_mm: 3.0,
        });
        assert_eq!(custom.step(true), k("Alt+Right"));
        assert!(custom.step(false).is_empty());
        assert_eq!(custom.step_mm(2.0), 3.0);
        assert_eq!(EdgeTarget::Volume.step_mm(2.5), 2.5);
    }

    #[test]
    fn sensitivity_defaults_are_inside_their_ranges_and_clamp_reports() {
        let mut d = Sensitivity::default();
        let before = d.clone();
        d.clamp();
        assert_eq!(d, before);
        let p = parse_text(
            "schema_version = 2\n[sensitivity]\ndrag_gain = 100.0\ndrag_start_mm = nan\nswipe_mm = 1.0\ntap_ms = 10\nhold_ms = 60000\npinch_out = inf\nflick_mm_s = 400.0\n",
        )
        .unwrap();
        let s = &p.config.sensitivity;
        assert_eq!(s.drag_gain, 40.0);
        assert_eq!(s.drag_start_mm, 1.5);
        assert_eq!(s.swipe_mm, 8.0);
        assert_eq!(s.tap_ms, 120);
        assert_eq!(s.hold_ms, 1500);
        assert_eq!(s.pinch_out, 1.3);
        assert_eq!(s.flick_mm_s, 400.0);
        assert_eq!(p.warnings.len(), 6, "{:?}", p.warnings);
        assert!(p.warnings.iter().any(|w| w.contains("60000")));
        assert!(!p.rewrite);
        // Every range holds the default.
        let r = Sensitivity::default();
        for (range, v) in [
            (Sensitivity::DRAG_GAIN, r.drag_gain),
            (Sensitivity::DRAG_START, r.drag_start_mm),
            (Sensitivity::WINDOW_START, r.window_start_mm),
            (Sensitivity::SWIPE, r.swipe_mm),
            (Sensitivity::TAP_TRAVEL, r.tap_travel_mm),
            (Sensitivity::TAP_MS, f32::from(r.tap_ms)),
            (Sensitivity::HOLD_TRAVEL, r.hold_travel_mm),
            (Sensitivity::HOLD_MS, f32::from(r.hold_ms)),
            (Sensitivity::WINDOW_GAP, f32::from(r.window_gap_ms)),
            (Sensitivity::PINCH_IN, r.pinch_in),
            (Sensitivity::PINCH_OUT, r.pinch_out),
            (Sensitivity::FLICK, r.flick_mm_s),
            (Sensitivity::EDGE_STRIP, r.edge_strip_mm),
            (Sensitivity::EDGE_DETENT, r.edge_detent_mm),
        ] {
            assert!((range.min..=range.max).contains(&v), "{v}");
        }
    }

    #[test]
    fn partial_file_fills_defaults() {
        let cfg: Config = toml::from_str("[drag]\nenabled = true\n").unwrap();
        assert!(cfg.drag.enabled);
        assert_eq!(cfg.drag.fingers, 3);
        assert_eq!(cfg.press, Press::default());
        assert_eq!(cfg.sensitivity, Sensitivity::default());
    }

    #[test]
    fn deep_stays_above_light() {
        let mut cfg = Config::default();
        cfg.press.light_threshold = 0.19;
        cfg.press.deep_threshold = 0.12;
        cfg.normalize();
        assert!(cfg.press.deep_threshold - cfg.press.light_threshold >= MIN_THRESHOLD_GAP - 1e-6);
        assert!(cfg.press.deep_threshold <= THRESHOLD_RANGE.1);
        cfg.press.light_threshold = 0.2;
        cfg.normalize();
        assert!(cfg.press.deep_threshold > cfg.press.light_threshold);
    }

    #[test]
    fn ranges_are_clamped() {
        let mut cfg = Config::default();
        cfg.drag.fingers = 9;
        cfg.drag.lift_timeout_ms = 5000;
        cfg.sensitivity.hold_ms = 5;
        cfg.press.fingers.clear();
        cfg.corner.corners.truncate(1);
        cfg.edge.left = EdgeTarget::Custom(CustomEdge {
            step_mm: f32::NAN,
            ..CustomEdge::default()
        });
        cfg.normalize();
        assert_eq!(cfg.drag.fingers, 5);
        assert_eq!(cfg.drag.lift_timeout_ms, 1000);
        assert_eq!(cfg.sensitivity.hold_ms, 300);
        assert_eq!(cfg.press.fingers.len(), 5);
        assert_eq!(cfg.corner.corners.len(), 4);
        assert!(matches!(&cfg.edge.left, EdgeTarget::Custom(c) if c.step_mm == 2.0));
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
        // Defaults: four-finger gestures and the ring menu are on; edge sliders are off.
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
        assert!(!list.iter().any(|(n, _)| n.contains("をスライド")));
        assert!(list.contains(&("4本指で左にスワイプ".into(), "次のデスクトップ".into())));
        cfg.edge.enabled = true;
        cfg.edge.left = EdgeTarget::Brightness;
        assert!(
            cfg.assignments()
                .contains(&("左端をスライド".into(), "明るさ".into()))
        );
        cfg.ring.enabled = false;
        cfg.edge.enabled = false;
        cfg.four.enabled = false;
        assert!(cfg.assignments().is_empty());
    }

    #[test]
    fn four_defaults_and_old_files() {
        // A file from before `[four]` existed gets the four-finger defaults.
        let cfg: Config = toml::from_str("[five]\nenabled = true\n").unwrap();
        assert_eq!(cfg.four, Four::default());
        assert!(cfg.four.enabled);
        assert_eq!(cfg.four.swipe_left, Action::DesktopNext);
        assert_eq!(cfg.four.swipe_right, Action::DesktopPrev);
        assert_eq!(cfg.four.swipe_up, Action::TaskView);
        assert_eq!(cfg.four.swipe_down, Action::ShowDesktop);
        assert_eq!(cfg.four.tap, Action::None);
        assert_eq!(cfg.four.pinch_in, Action::None);
        assert_eq!(cfg.four.pinch_out, Action::None);
        // A partial `[four]` keeps the other defaults.
        let cfg: Config = toml::from_str("[four]\nswipe_up = \"none\"\n").unwrap();
        assert_eq!(cfg.four.swipe_up, Action::None);
        assert_eq!(cfg.four.swipe_left, Action::DesktopNext);
    }

    #[test]
    fn desktop_actions_have_tokens_and_labels() {
        assert_eq!(Action::DesktopPrev.to_token(), "desktop_prev");
        assert_eq!(Action::from_token("desktop_next"), Action::DesktopNext);
        assert_eq!(Action::DesktopPrev.label(), "前のデスクトップ");
        assert_eq!(Action::DesktopNext.label(), "次のデスクトップ");
    }

    #[test]
    fn partial_profile_and_ring_item_load() {
        let cfg: Config =
            toml::from_str("[[profiles]]\nexe_name = \"a.exe\"\n[[ring.items]]\nlabel = \"x\"\n")
                .unwrap();
        assert_eq!(cfg.profiles[0].exe_name, "a.exe");
        assert!(!cfg.profiles[0].enabled);
        assert!(cfg.profiles[0].overrides.is_empty());
        assert_eq!(cfg.ring.items[0].label, "x");
        assert_eq!(cfg.ring.items[0].action, Action::None);
    }

    #[test]
    fn duplicate_profiles_keep_the_first_and_are_reported() {
        let text = "schema_version = 2\n[[profiles]]\nexe_name = \"Code.exe\"\nenabled = true\n[[profiles]]\nexe_name = \"code.EXE\"\n[[profiles]]\nexe_name = \"other.exe\"\n[[profiles]]\n[[profiles]]\n";
        let p = parse_text(text).unwrap();
        let names: Vec<_> = p
            .config
            .profiles
            .iter()
            .map(|p| p.exe_name.as_str())
            .collect();
        assert_eq!(names, ["Code.exe", "other.exe", "", ""]);
        assert!(p.config.profiles[0].enabled);
        assert_eq!(p.duplicate_profiles, vec!["code.EXE".to_owned()]);
    }

    #[test]
    fn duplicate_profiles_compare_by_lowercased_basename() {
        let text = "schema_version = 2
[[profiles]]
exe_name = \"code.exe\"
[[profiles]]
exe_name = 'C:\\Apps\\Code.exe'
";
        let p = parse_text(text).unwrap();
        assert_eq!(p.config.profiles.len(), 1);
        assert_eq!(p.duplicate_profiles.len(), 1);
    }

    #[test]
    fn wrong_typed_sensitivity_values_do_not_reset_the_file() {
        let text = "schema_version = 2
gestures_enabled = false
[sensitivity]
hold_ms = 70000
tap_ms = -5
drag_gain = \"fast\"
swipe_mm = 20
";
        let p = parse_text(text).unwrap();
        let s = &p.config.sensitivity;
        assert!(!p.config.gestures_enabled);
        assert_eq!(s.hold_ms, 1500);
        assert_eq!(s.tap_ms, 120);
        assert_eq!(s.drag_gain, Sensitivity::default().drag_gain);
        assert_eq!(s.swipe_mm, 20.0);
        assert!(p.warnings.iter().any(|w| w.contains("数値ではない")));
        // The clamp report shows the number from the file, not the saturated stored one.
        assert!(
            p.warnings.iter().any(|w| w.contains("70000 は範囲外")),
            "{:?}",
            p.warnings
        );
        assert!(
            p.warnings.iter().any(|w| w.contains("-5 は範囲外")),
            "{:?}",
            p.warnings
        );
        assert!(!p.warnings.iter().any(|w| w.contains("65535")));
        let p = parse_text(
            "schema_version = 2
sensitivity = 5
",
        )
        .unwrap();
        assert_eq!(p.config.sensitivity, Sensitivity::default());
        assert_eq!(p.warnings.len(), 1);
    }

    #[test]
    fn wrong_typed_action_edge_and_finger_values_do_not_reset_the_file() {
        let p = parse_text(
            "schema_version = 2
gestures_enabled = false
[four]
swipe_up = 5
swipe_down = \"find\"
[edge]
left = 5
right = \"volume\"
[drag]
fingers = \"3\"
[long_hold]
fingers = true
[ring]
fingers = [1]
",
        )
        .unwrap();
        let c = &p.config;
        assert!(!c.gestures_enabled);
        assert_eq!(c.four.swipe_up, Action::None);
        assert_eq!(c.four.swipe_down, Action::Find);
        assert_eq!(c.edge.left, EdgeTarget::None);
        assert_eq!(c.edge.right, EdgeTarget::Volume);
        assert_eq!(c.drag.fingers, Drag::default().fingers);
        assert_eq!(c.long_hold.fingers, LongHold::default().fingers);
        assert_eq!(c.ring.fingers, Ring::default().fingers);
        assert!(p.unknown_tokens.is_empty());
        assert_eq!(p.warnings.len(), 5, "{:?}", p.warnings);
    }

    #[test]
    fn corner_and_hold_overrides_are_dropped_with_a_note() {
        let p = parse_text(
            "schema_version = 2
[[profiles]]
exe_name = \"a.exe\"
[profiles.overrides]
\"corner.br.swipe\" = \"find\"
\"hold.long\" = \"find\"
\"hold.thumb.index.swipe\" = \"find\"
\"press.3.deep\" = \"find\"
\"swipe.4.left\" = \"none\"
",
        )
        .unwrap();
        let keys: Vec<_> = p.config.profiles[0].overrides.keys().cloned().collect();
        assert_eq!(keys, ["press.3.deep", "swipe.4.left"]);
        assert_eq!(p.inert_overrides.len(), 3);
        assert!(p.unbindable_overrides.is_empty());
        assert!(p.warnings.iter().any(|w| w.contains("コーナー・ホールド")));
    }

    #[test]
    fn non_table_ring_item_is_reported_on_migration() {
        let p = parse_text(
            "[ring]
items = [5, { label = \"ok\", command = \"ctrl+c\" }]
",
        )
        .unwrap();
        assert_eq!(p.config.ring.items.len(), 1);
        assert!(
            p.warnings.iter().any(|w| w.contains("表の形ではない")),
            "{:?}",
            p.warnings
        );
    }

    #[test]
    fn custom_edge_step_clamps_are_reported() {
        let text = "schema_version = 2
[edge.left.custom]
step_mm = 99.0
";
        let p = parse_text(text).unwrap();
        assert!(
            p.warnings.iter().any(|w| w.contains("左端")),
            "{:?}",
            p.warnings
        );
        let text = "schema_version = 2
[[profiles]]
exe_name = \"a.exe\"
[profiles.edge_overrides.\"edge.top\".custom]
step_mm = 0.1
";
        let p = parse_text(text).unwrap();
        assert!(
            p.warnings.iter().any(|w| w.contains("a.exe")),
            "{:?}",
            p.warnings
        );
    }

    #[test]
    fn unbindable_override_ids_are_dropped_and_reported() {
        let text = "schema_version = 2
[[profiles]]
exe_name = \"a.exe\"
[profiles.overrides]
\"swipe.4.left\" = \"none\"
\"tap.5\" = \"none\"
\"swipe.3.left\" = \"none\"
\"pinch.3.in\" = \"none\"
";
        let p = parse_text(text).unwrap();
        let keys: Vec<_> = p.config.profiles[0].overrides.keys().cloned().collect();
        assert_eq!(keys, ["swipe.4.left"]);
        assert_eq!(p.unbindable_overrides.len(), 3);
        assert!(p.warnings.iter().any(|w| w.contains("a.exe")));
    }

    #[test]
    fn non_string_ring_command_is_reported_as_dropped() {
        let text = "[[ring.items]]
label = \"x\"
command = 5
";
        let p = parse_text(text).unwrap();
        assert!(p.config.ring.items.is_empty());
        assert!(p.warnings.iter().any(|w| w.contains("「x」")));
    }

    #[test]
    fn bad_override_keys_are_dropped() {
        let text = "schema_version = 2\n[[profiles]]\nexe_name = \"a.exe\"\n[profiles.overrides]\n\"swipe.4.left\" = \"none\"\n\"swipe.9.left\" = \"none\"\n[profiles.edge_overrides]\n\"edge.left\" = \"scroll\"\n\"edge.bottom\" = \"scroll\"\n";
        let p = parse_text(text).unwrap();
        let prof = &p.config.profiles[0];
        assert_eq!(prof.overrides.len(), 1);
        assert_eq!(prof.edge_overrides.len(), 1);
        assert!(p.warnings.iter().any(|w| w.contains("a.exe")));
    }

    #[test]
    fn invalid_toml_is_moved_aside_without_overwriting_backups() {
        let dir = temp_dir("badtoml");
        let path = dir.join("config.toml");
        for _ in 0..2 {
            fs::write(&path, "this is = = not toml").unwrap();
            let got = load_report(&path);
            assert_eq!(got.config, Config::default());
            assert_eq!(got.warnings.len(), 1);
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

    #[test]
    fn gesture_ids_round_trip() {
        let all = GestureId::all();
        let mut tokens: Vec<_> = all.iter().map(|i| i.as_str().to_owned()).collect();
        let n = tokens.len();
        tokens.sort();
        tokens.dedup();
        assert_eq!(tokens.len(), n, "ids are unique");
        assert_eq!(n, 3 + 3 * 6 + 10 + 1 + 8 + 20);
        for id in &all {
            assert_eq!(GestureId::parse(id.as_str()).as_ref(), Some(id));
            assert_eq!(id.to_string(), id.as_str());
        }
        assert_eq!(GestureId::parse("swipe.4.sideways"), None);
        assert_eq!(GestureId::parse(""), None);
        assert_eq!(GestureId::tap(4).as_str(), "tap.4");
        assert_eq!(GestureId::press(2, true).as_str(), "press.2.deep");
        assert_eq!(GestureId::corner(3, true).as_str(), "corner.br.swipe");
        assert_eq!(
            GestureId::hold_combo(true, Finger::Little, HoldTrigger::Tap).as_str(),
            "hold.thumb.little.tap"
        );
    }

    /// Every id is either backed by exactly one config field (bind and read back) or has no
    /// field at all; and no Action field of the config is missed by the ids.
    #[test]
    fn every_action_field_is_reachable_through_a_gesture_id() {
        let mut cfg = Config::default();
        let mut bound = 0;
        for (i, id) in GestureId::all().iter().enumerate() {
            let sentinel = Action::Keys(format!("sentinel-{i}"));
            let ok = cfg.set_binding(id, sentinel.clone());
            assert_eq!(cfg.binding(id).is_some(), ok, "{id}");
            if ok {
                bound += 1;
                assert_eq!(cfg.binding(id), Some(sentinel), "{id}");
            }
        }
        // 7 (four) + 4 (five) + 10 (press) + 1 (long hold) + 8 (corners) + 20 (hold combos).
        assert_eq!(bound, 50);
        // Ids without a field refuse.
        for id in ["tap.3", "tap.5", "swipe.3.up", "swipe.5.left", "pinch.3.in"] {
            let id = GestureId::parse(id).unwrap();
            assert_eq!(cfg.binding(&id), None);
            assert!(!cfg.set_binding(&id, Action::Find));
        }
        // Walk the saved file: every action-valued entry (outside [edge]) is a sentinel.
        let value = toml::Value::try_from(&cfg).unwrap();
        fn walk(v: &toml::Value, path: &str, out: &mut Vec<(String, String)>) {
            match v {
                toml::Value::String(s) => out.push((path.to_owned(), s.clone())),
                toml::Value::Table(t) => t
                    .iter()
                    .for_each(|(k, v)| walk(v, &format!("{path}.{k}"), out)),
                toml::Value::Array(a) => a.iter().for_each(|v| walk(v, path, out)),
                _ => {}
            }
        }
        let mut strings = Vec::new();
        walk(&value, "", &mut strings);
        let mut sentinels = 0;
        for (path, s) in strings {
            if path.starts_with(".edge.") {
                continue;
            }
            if let Some(rest) = s.strip_prefix("keys:") {
                assert!(rest.starts_with("sentinel-"), "{path} = {s}");
                sentinels += 1;
            } else if path.ends_with(".keys") {
                assert!(s.starts_with("sentinel-"), "{path} = {s}");
                sentinels += 1;
            } else {
                assert!(
                    !matches!(Action::parse_token(&s), TokenParse::Ok(_)),
                    "action field not reachable through a gesture id: {path} = {s}"
                );
            }
        }
        assert_eq!(sentinels, 50);
    }

    #[test]
    fn hold_combo_bindings_accept_only_keys() {
        let mut cfg = Config::default();
        let id = GestureId::hold_combo(false, Finger::Index, HoldTrigger::Swipe);
        assert_eq!(cfg.binding(&id), Some(Action::None));
        assert!(!cfg.set_binding(&id, Action::TaskView));
        assert!(cfg.set_binding(&id, Action::Keys("Alt+Tab".into())));
        assert_eq!(cfg.finger_hold.combos.len(), 1);
        assert!(cfg.set_binding(&id, Action::Keys("Ctrl+Tab".into())));
        assert_eq!(cfg.finger_hold.combos.len(), 1);
        assert_eq!(cfg.binding(&id), Some(Action::Keys("Ctrl+Tab".into())));
        assert!(cfg.set_binding(&id, Action::None));
        assert_eq!(cfg.binding(&id), Some(Action::None));
    }

    fn calib(b: f32, f1: f32, f2: Option<f32>, f3: Option<f32>) -> Calibration {
        Calibration {
            baseline: b,
            f1,
            f2,
            f3,
            f4: None,
            f5: None,
            date: "2026-11-15".into(),
        }
    }

    #[test]
    fn defaults_are_the_raw_thresholds() {
        let cfg = Config::default();
        assert_eq!(
            (cfg.press.light_threshold, cfg.press.deep_threshold),
            (0.08, 0.14)
        );
        assert_eq!(cfg.press.thresholds(1), (0.08, 0.14));
        // Two to five fingers start with their own, lower pair ("same as one finger" is off).
        for n in 2..=5 {
            assert_eq!(cfg.press.thresholds(n), (0.07, 0.12), "{n}");
            assert!(cfg.press.fingers[n - 1].light_threshold.is_some());
        }
        let mut c = cfg.clone();
        c.normalize();
        assert_eq!(c.press, cfg.press);
    }

    #[test]
    fn thresholds_keep_the_range_and_the_gap() {
        let mut cfg = Config::default();
        cfg.press.light_threshold = 0.19;
        cfg.press.deep_threshold = 0.2;
        cfg.normalize();
        assert_eq!(
            (cfg.press.light_threshold, cfg.press.deep_threshold),
            (0.18, 0.2)
        );
        cfg.press.light_threshold = 0.05;
        cfg.press.deep_threshold = 0.06;
        cfg.normalize();
        assert_eq!(
            (cfg.press.light_threshold, cfg.press.deep_threshold),
            (0.05, 0.07)
        );
        for (l, d) in [(0.01, 0.2), (0.01, 0.03), (0.18, 0.2)] {
            assert_eq!(fit_pair(l, d), (l, d));
        }
    }

    #[test]
    fn old_calibrated_scale_values_migrate_or_fall_back_with_a_note() {
        // In range: kept as the one-finger thresholds.
        let raw = parse_text("[press]\nlight_threshold = 0.09\ndeep_threshold = 0.17\n").unwrap();
        assert_eq!(
            (
                raw.config.press.light_threshold,
                raw.config.press.deep_threshold
            ),
            (0.09, 0.17)
        );
        // Out of range (the old 0..1 scale): defaults, and the load says so.
        let old = parse_text("[press]\nlight_threshold = 0.3\ndeep_threshold = 0.7\n").unwrap();
        assert_eq!(
            (
                old.config.press.light_threshold,
                old.config.press.deep_threshold
            ),
            DEFAULT_ONE
        );
        assert!(
            old.warnings.iter().any(|w| w.contains("しきい値")),
            "{:?}",
            old.warnings
        );
        // One side out of range: the whole pair falls back, not just that side.
        let half = parse_text("[press]\nlight_threshold = 0.09\ndeep_threshold = 0.7\n").unwrap();
        assert_eq!(
            (
                half.config.press.light_threshold,
                half.config.press.deep_threshold
            ),
            DEFAULT_ONE
        );
        assert!(
            half.warnings.iter().any(|w| w.contains("しきい値")),
            "{:?}",
            half.warnings
        );
    }

    #[test]
    fn a_press_count_without_enabled_reads_as_on() {
        // A file from before `enabled` existed: the entries only have their bindings.
        let old = parse_text(
            "[press]
enabled = true
[[press.fingers]]
light = \"Find\"
[[press.fingers]]
",
        )
        .unwrap();
        assert!(old.config.press.fingers[0].enabled);
        assert!(old.config.press.fingers[1].enabled);
        // A deliberately off count survives a save and a load.
        let mut cfg = Config::default();
        cfg.press.fingers[0].enabled = true;
        cfg.press.fingers[2].enabled = false;
        let back: Config = toml::from_str(&toml::to_string_pretty(&cfg).unwrap()).unwrap();
        assert!(back.press.fingers[0].enabled);
        assert!(!back.press.fingers[2].enabled);
        // A new default config keeps its defaults (everything off).
        assert!(Config::default().press.fingers.iter().all(|f| !f.enabled));
    }

    #[test]
    fn quantisation_writes_clean_two_decimal_values() {
        assert_eq!(q01(0.15 + 1e-7), 0.15);
        assert_eq!(q01(0.1 + 0.05), 0.15);
        let mut cfg = Config::default();
        cfg.press.light_threshold = 0.07 + 1e-7;
        cfg.press.deep_threshold = 0.14 - 1e-7;
        cfg.press.fingers[2].light_threshold = Some(0.06 + 2e-7);
        cfg.press.fingers[2].deep_threshold = Some(0.11 - 2e-7);
        cfg.calibration.insert(
            "05ac:0324".into(),
            calib(0.01 + 1e-7, 0.2 - 1e-7, Some(0.16), None),
        );
        cfg.normalize();
        let text = toml::to_string_pretty(&cfg).unwrap();
        for want in [
            "light_threshold = 0.07\n",
            "deep_threshold = 0.14\n",
            "light_threshold = 0.06\n",
            "deep_threshold = 0.11\n",
            "baseline = 0.01\n",
            "f1 = 0.2\n",
            "f2 = 0.16\n",
        ] {
            assert!(text.contains(want), "{want:?} missing in {text}");
        }
        let back: Config = toml::from_str(&text).unwrap();
        assert_eq!(back.calibration, cfg.calibration);
        assert_eq!(back.press, cfg.press);
    }

    #[test]
    fn per_count_normalize_drops_bad_calibration_entries() {
        let mut cfg = Config::default();
        cfg.calibration.insert(
            "05ac:0324".into(),
            calib(0.01, 0.20, Some(0.16), Some(0.02)),
        );
        cfg.calibration
            .insert("1111:2222".into(), calib(0.01, 0.03, None, None));
        cfg.normalize();
        assert!(!cfg.calibration.contains_key("1111:2222"));
        let c = &cfg.calibration["05ac:0324"];
        assert!(c.usable(1) && c.usable(2));
        // The 3-finger level is too close to the baseline: dropped.
        assert!(!c.usable(3) && c.f3.is_none());
        assert!(!c.usable(4) && !c.usable(5));
    }

    #[test]
    fn per_count_thresholds_fall_back_and_keep_the_gap() {
        let mut cfg = Config::default();
        cfg.press.light_threshold = 0.09;
        cfg.press.deep_threshold = 0.15;
        // Counts 3 and 4 share the one-finger pair.
        for n in [3, 4] {
            cfg.press.fingers[n - 1].light_threshold = None;
            cfg.press.fingers[n - 1].deep_threshold = None;
        }
        cfg.press.fingers[1].light_threshold = Some(0.10);
        cfg.press.fingers[1].deep_threshold = Some(0.11);
        cfg.press.fingers[4].light_threshold = Some(0.05);
        cfg.press.fingers[4].deep_threshold = None;
        cfg.normalize();
        assert_eq!(cfg.press.thresholds(1), (0.09, 0.15));
        assert_eq!(cfg.press.thresholds(3), (0.09, 0.15));
        assert_eq!(cfg.press.thresholds(4), (0.09, 0.15));
        // A too-close pair is pushed apart per count.
        assert_eq!(cfg.press.thresholds(2), (0.10, 0.12));
        // One value given: the other follows the one-finger setting.
        assert_eq!(cfg.press.thresholds(5), (0.05, 0.15));
        // One finger never reads its own override.
        cfg.press.fingers[0].light_threshold = Some(0.5);
        cfg.normalize();
        assert_eq!(cfg.press.thresholds(1), (0.09, 0.15));
        assert_eq!(cfg.press.fingers[0].light_threshold, None);
    }

    #[test]
    fn a_count_needs_no_calibration_to_be_active() {
        let mut cfg = Config::default();
        assert!(!cfg.press.count_active(1));
        cfg.press.enabled = true;
        assert!(!cfg.press.count_active(1));
        for f in &mut cfg.press.fingers {
            f.enabled = true;
        }
        assert!((1..=5).all(|n| cfg.press.count_active(n)));
        assert!(cfg.calibration.is_empty());
        cfg.press.fingers[1].enabled = false;
        assert!(!cfg.press.count_active(2));
        cfg.press.enabled = false;
        assert!(!cfg.press.count_active(1));
    }

    #[test]
    fn cursor_speed_defaults_to_off_and_round_trips() {
        let c = Config::default();
        assert_eq!(c.cursor.speed, None);
        let text = toml::to_string(&c).unwrap();
        assert!(!text.contains("speed"), "{text}");
        let mut c = Config::default();
        c.cursor.speed = Some(12);
        c.profiles.push(Profile {
            exe_name: "a.exe".into(),
            cursor_speed: Some(3),
            ..Profile::default()
        });
        let back = parse_text(&toml::to_string(&c).unwrap()).unwrap();
        assert_eq!(back.config.cursor.speed, Some(12));
        assert_eq!(back.config.profiles[0].cursor_speed, Some(3));
        assert_eq!(back.config.schema_version, 2);
    }

    #[test]
    fn cursor_speed_out_of_range_is_clamped_and_reported() {
        let p = parse_text(
            "schema_version = 2
[cursor]
speed = 99
[[profiles]]
exe_name = \"a.exe\"
cursor_speed = 0
[[profiles]]
exe_name = \"b.exe\"
cursor_speed = \"fast\"
",
        )
        .unwrap();
        assert_eq!(p.config.cursor.speed, Some(20));
        assert_eq!(p.config.profiles[0].cursor_speed, Some(1));
        assert_eq!(p.config.profiles[1].cursor_speed, None);
        assert!(
            p.warnings.iter().any(|w| w.contains("99 は範囲外")),
            "{:?}",
            p.warnings
        );
        assert!(
            p.warnings.iter().any(|w| w.contains("0 は範囲外")),
            "{:?}",
            p.warnings
        );
        assert!(
            p.warnings.iter().any(|w| w.contains("整数ではない")),
            "{:?}",
            p.warnings
        );
        let big = parse_text(
            "schema_version = 2
[cursor]
speed = 300
",
        )
        .unwrap();
        assert!(
            big.warnings.iter().any(|w| w.contains("300 は範囲外")),
            "{:?}",
            big.warnings
        );
    }
}
