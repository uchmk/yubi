//! Work behind the 全般 page: settings export / import and the haptic permission check / grant.
//!
//! Everything that blocks (file dialogs, file IO, the elevated helper) runs on a short-lived worker
//! thread and reports back over a channel; the page only reads [`GeneralState`] and calls its
//! `start_*` methods. The import is applied on the UI thread (a few small file writes).

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};

use eframe::egui;

use crate::config::{self, Config};
use crate::config_io::{self, Imported};
use crate::haptic::permission::{self, PermissionError, PermissionState};
use crate::os::file_dialog;

/// Why 適用 and 元に戻す are off while the config file came from a newer yubi.
pub const READ_ONLY_REASON: &str = "新しい版の yubi で作られた設定なので、書き換えません";

pub const DEFAULT_EXPORT_NAME: &str = "yubi-settings.toml";

/// What the haptic permission row shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HapticStatus {
    /// A check or the elevated step is running.
    Checking,
    Granted,
    NotGranted,
    /// The driver's registry keys are missing, so nothing can be said.
    Unknown,
}

/// A line under a section: what happened last, in Japanese.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    pub text: String,
    pub is_error: bool,
}

impl Note {
    fn ok(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            is_error: false,
        }
    }

    fn err(text: impl Into<String>) -> Self {
        Self {
            text: format!("エラー: {}", text.into()),
            is_error: true,
        }
    }
}

/// One section of the config whose values would change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub section: String,
    pub count: usize,
    /// A few `name: before → after` lines (the rest is only counted).
    pub details: Vec<String>,
}

/// A parsed import waiting for 適用 / キャンセル.
#[derive(Debug, Clone)]
pub struct Preview {
    /// The live config this preview was compared with; 適用 only runs while it is still the live one.
    base: Config,
    pub path: PathBuf,
    pub imported: Imported,
    pub changes: Vec<Change>,
}

/// What 元に戻す restores.
#[derive(Debug, Clone)]
struct Undo {
    previous: Config,
    backup: Option<PathBuf>,
}

enum GrantResult {
    Done { restart_requested: bool },
    Cancelled,
    Failed(String),
}

enum Msg {
    Exported(Result<Option<PathBuf>, String>),
    Picked(Box<Result<Option<(PathBuf, Imported)>, String>>),
    Checked(HapticStatus),
    Granted(GrantResult, HapticStatus),
}

pub struct GeneralState {
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    ctx: egui::Context,
    /// A dialog or file operation is running on a worker.
    pub io_busy: bool,
    pub io_note: Option<Note>,
    pub preview: Option<Preview>,
    undo: Option<Undo>,
    pub haptic: HapticStatus,
    pub haptic_note: Option<Note>,
    visible: bool,
    /// What loading the config file reported (migration, dropped entries, clamped values), shown
    /// once at the top of the page until dismissed.
    pub load_notes: Vec<String>,
    /// The config file was written by a newer yubi: nothing may overwrite it.
    pub read_only: bool,
}

impl GeneralState {
    pub fn new(ctx: &egui::Context) -> Self {
        let (tx, rx) = channel();
        Self {
            tx,
            rx,
            ctx: ctx.clone(),
            io_busy: false,
            load_notes: Vec::new(),
            read_only: false,
            io_note: None,
            preview: None,
            undo: None,
            haptic: HapticStatus::Checking,
            haptic_note: None,
            visible: false,
        }
    }

    pub fn can_undo(&self) -> bool {
        self.undo.is_some()
    }

    /// Called every frame with whether the 全般 page is showing; re-checks the permission when it opens.
    pub fn set_visible(&mut self, visible: bool) {
        if visible && !self.visible {
            self.start_haptic_check();
        }
        self.visible = visible;
    }

    fn spawn(&self, work: impl FnOnce() -> Msg + Send + 'static) {
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(work());
            ctx.request_repaint();
        });
    }

    // ---- export / import -------------------------------------------------------------------

    pub fn start_export(&mut self, cfg: &Config) {
        if self.io_busy {
            return;
        }
        self.io_busy = true;
        self.io_note = None;
        let cfg = cfg.clone();
        self.spawn(move || Msg::Exported(export_job(&cfg)));
    }

    pub fn start_import(&mut self) {
        if self.io_busy {
            return;
        }
        self.io_busy = true;
        self.io_note = None;
        self.spawn(|| Msg::Picked(Box::new(import_job())));
    }

    pub fn cancel_preview(&mut self) {
        self.preview = None;
        self.io_note = None;
    }

    /// Replace the live config with the previewed one: save the current config, back it up, then set
    /// the new one (the app publishes it to the engine as one generation, which cancels any active
    /// gesture) and save it.
    pub fn apply_preview(&mut self, cfg: &mut Config, config_path: &Path) {
        let Some(mut p) = self.preview.take() else {
            return;
        };
        if self.read_only {
            self.preview = Some(p);
            self.io_note = Some(Note::err(READ_ONLY_REASON));
            return;
        }
        if p.base != *cfg {
            // Edited (or autostart toggled) while the preview was open: compare again and ask again.
            let path = p.path.clone();
            self.preview = Some(make_preview(cfg, path, &mut p.imported));
            self.io_note = Some(Note::ok(
                "プレビューを開いている間に設定が変更されたため、変更内容を更新しました。もう一度確認してください。",
            ));
            return;
        }
        match apply_import(cfg, config_path, &p.imported.config, self.read_only) {
            Ok(undo) => {
                self.undo = Some(undo);
                self.io_note = Some(Note::ok(
                    "設定を読み込みました。「元に戻す」で、読み込み前の状態にすべての設定を戻せます（その後の変更は失われます）。",
                ));
            }
            Err(e) => {
                self.preview = Some(p);
                self.io_note = Some(Note::err(e));
            }
        }
    }

    pub fn undo_import(&mut self, cfg: &mut Config, config_path: &Path) {
        let Some(u) = self.undo.take() else {
            return;
        };
        match undo_apply(cfg, config_path, &u, self.read_only) {
            Ok(()) => self.io_note = Some(Note::ok("読み込み前の設定に戻しました。")),
            Err(e) => {
                self.undo = Some(u);
                self.io_note = Some(Note::err(e));
            }
        }
    }

    // ---- haptic permission -----------------------------------------------------------------

    fn start_haptic_check(&mut self) {
        if let Some(forced) = shot_override() {
            self.haptic = forced;
            return;
        }
        self.haptic = HapticStatus::Checking;
        self.spawn(|| Msg::Checked(read_status()));
    }

    /// Launch the elevated step (UAC prompt). Only ever called from the button.
    pub fn start_haptic_grant(&mut self) {
        if self.haptic == HapticStatus::Checking {
            return;
        }
        self.haptic = HapticStatus::Checking;
        self.haptic_note = None;
        self.spawn(|| {
            let result = match permission::enable_non_admin_access() {
                Ok(out) => GrantResult::Done {
                    restart_requested: !out.restart_requested.is_empty(),
                },
                Err(PermissionError::Cancelled) => GrantResult::Cancelled,
                Err(PermissionError::Failed(m)) => GrantResult::Failed(m),
            };
            Msg::Granted(result, read_status())
        });
    }

    // ---- worker results --------------------------------------------------------------------

    /// Take what the workers have reported. `cfg` is the live config (the import is compared with it).
    pub fn poll(&mut self, cfg: &Config) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Exported(r) => {
                    self.io_busy = false;
                    self.io_note = match r {
                        Ok(Some(p)) => Some(Note::ok(format!("書き出しました: {}", p.display()))),
                        Ok(None) => None,
                        Err(e) => Some(Note::err(e)),
                    };
                }
                Msg::Picked(r) => {
                    let r = *r;
                    self.io_busy = false;
                    match r {
                        Ok(Some((path, mut imported))) => {
                            self.preview = Some(make_preview(cfg, path, &mut imported));
                            self.io_note = None;
                        }
                        Ok(None) => {}
                        Err(e) => self.io_note = Some(Note::err(e)),
                    }
                }
                Msg::Checked(s) => self.haptic = s,
                Msg::Granted(r, status) => {
                    self.haptic = status;
                    self.haptic_note = Some(match r {
                        GrantResult::Done {
                            restart_requested: true,
                        } => Note::ok(
                            "許可しました。反映されないときは、トラックパッドを接続し直してください。",
                        ),
                        GrantResult::Done {
                            restart_requested: false,
                        } => Note::ok("許可しました。トラックパッドを接続し直すと反映されます。"),
                        GrantResult::Cancelled => Note::ok("許可はキャンセルされました。"),
                        GrantResult::Failed(m) => Note::err(m),
                    });
                }
            }
        }
    }

    /// Screenshot helper (`YUBI_SHOT_GENERAL=preview` or `readonly`): show a sample import preview
    /// (`readonly` also switches the page to the newer-file state).
    pub fn demo(&mut self, cfg: &Config) {
        if std::env::var_os("YUBI_SHOT").is_none() || self.preview.is_some() {
            return;
        }
        match std::env::var("YUBI_SHOT_GENERAL").as_deref() {
            Ok("preview") => {}
            Ok("readonly") => self.read_only = true,
            _ => return,
        }
        let mut sample = cfg.clone();
        sample.drag.four.enabled = true;
        sample.sensitivity.hold_ms = 1500;
        sample.press.light_threshold = 0.1;
        sample.five.swipe_up = config::Action::Keys("Win+Tab".into());
        let mut imported = Imported {
            config: sample,
            warnings: vec![
                "長押しの時間 (ms) の値 60000 は範囲外のため 2000 に補正しました".into(),
            ],
        };
        self.preview = Some(make_preview(
            cfg,
            PathBuf::from(r"C:\Users\user\Documents\yubi-settings.toml"),
            &mut imported,
        ));
    }
}

fn export_job(cfg: &Config) -> Result<Option<PathBuf>, String> {
    let Some(path) = file_dialog::save_toml(DEFAULT_EXPORT_NAME).map_err(|e| e.message())? else {
        return Ok(None);
    };
    config_io::export_to(&path, cfg).map_err(|e| e.to_string())?;
    Ok(Some(path))
}

fn import_job() -> Result<Option<(PathBuf, Imported)>, String> {
    let Some(path) = file_dialog::open_toml().map_err(|e| e.message())? else {
        return Ok(None);
    };
    let imported = config_io::import_from(&path).map_err(|e| e.to_string())?;
    Ok(Some((path, imported)))
}

// ---- pure logic (unit tested) ---------------------------------------------------------------

/// The imported file never switches autostart (it is a registry/task change, not a setting to copy).
fn make_preview(cfg: &Config, path: PathBuf, imported: &mut Imported) -> Preview {
    imported.config.general.start_with_windows = cfg.general.start_with_windows;
    // The calibration belongs to this trackpad, not to the file.
    imported.config.calibration = cfg.calibration.clone();
    let changes = diff(cfg, &imported.config);
    Preview {
        base: cfg.clone(),
        path,
        imported: imported.clone(),
        changes,
    }
}

/// Back up the file that is on disk, then replace the live config with `new` and save it. Refuses
/// while the file came from a newer yubi (`read_only`).
fn apply_import(
    cfg: &mut Config,
    config_path: &Path,
    new: &Config,
    read_only: bool,
) -> Result<Undo, String> {
    if read_only {
        return Err(READ_ONLY_REASON.to_owned());
    }
    let dir = config_path.parent().unwrap_or_else(|| Path::new("."));
    let backup = config_io::backup_current(dir).map_err(|e| e.to_string())?;
    let previous = cfg.clone();
    config::save(config_path, new).map_err(|e| format!("設定を保存できません: {e}"))?;
    *cfg = new.clone();
    Ok(Undo { previous, backup })
}

/// Restore from the backup file (or the in-memory copy when there was nothing to back up).
fn undo_apply(
    cfg: &mut Config,
    config_path: &Path,
    u: &Undo,
    read_only: bool,
) -> Result<(), String> {
    if read_only {
        return Err(READ_ONLY_REASON.to_owned());
    }
    let mut restored = match &u.backup {
        Some(b) => config_io::restore_from(b)
            .map(|i| i.config)
            .map_err(|e| e.to_string())?,
        None => u.previous.clone(),
    };
    restored.general.start_with_windows = cfg.general.start_with_windows;
    restored.calibration = cfg.calibration.clone();
    config::save(config_path, &restored).map_err(|e| format!("設定を保存できません: {e}"))?;
    *cfg = restored;
    Ok(())
}

fn section_label(key: &str) -> &str {
    match key {
        "gestures_enabled" => "ジェスチャー全体",
        "general" => "全般",
        "drag" => "ドラッグ",
        "four" => "4本指ジェスチャー",
        "five" => "5本指ジェスチャー",
        "sensitivity" => "感度",
        "schema_version" => "設定ファイルの版",
        "press" => "押し込みと触覚",
        "calibration" => "押し込みのキャリブレーション",
        "long_hold" => "ロングホールド",
        "finger_hold" => "フィンガーホールド",
        "thumb_hold" => "サムホールド",
        "corner" => "コーナー操作",
        "edge" => "エッジスライダー",
        "window" => "ウィンドウ操作",
        "profiles" => "アプリ設定",
        "ring" => "リングメニュー",
        other => other,
    }
}

fn show_value(v: Option<&toml::Value>) -> String {
    match v {
        None => "なし".into(),
        Some(toml::Value::Array(a)) => format!("{} 件", a.len()),
        Some(toml::Value::Table(_)) => "…".into(),
        // The config stores f32; show it as one (not as the widened f64).
        Some(toml::Value::Float(f)) => format!("{}", *f as f32),
        Some(other) => other.to_string(),
    }
}

fn walk(
    path: &str,
    a: Option<&toml::Value>,
    b: Option<&toml::Value>,
    out: &mut Vec<(String, String)>,
) {
    if let (Some(toml::Value::Table(x)), Some(toml::Value::Table(y))) = (a, b) {
        let mut keys: Vec<&String> = x.keys().collect();
        keys.extend(y.keys().filter(|k| !x.contains_key(*k)));
        for k in keys {
            walk(&format!("{path}.{k}"), x.get(k), y.get(k), out);
        }
    } else if a != b {
        out.push((
            setting_label(path),
            format!("{} → {}", show_value(a), show_value(b)),
        ));
    }
}

/// The label the pages use for a setting (`section.key`); the key itself when there is none.
fn setting_label(path: &str) -> String {
    match path {
        "gestures_enabled" => "ジェスチャーを有効にする",
        "general.theme" => "テーマ",
        "general.start_with_windows" => "Windows の起動時に開始",
        "general.pause_shortcut" => "一時停止のショートカット",
        "general.pause_in_fullscreen" => "全画面のアプリの間は自動で一時停止",
        "five.enabled"
        | "press.enabled"
        | "long_hold.enabled"
        | "finger_hold.enabled"
        | "thumb_hold.enabled"
        | "corner.enabled"
        | "edge.enabled"
        | "ring.enabled" => "機能を有効にする",
        "drag.lift_timeout_ms" => "指を離してから終了するまでの時間",
        "drag.keep_drag_on_release" => "離したあともドラッグを続ける",
        "five.tap" => "タップ",
        "five.swipe_left" => "左スワイプ",
        "five.swipe_right" => "右スワイプ",
        "five.pinch_in" => "ピンチイン",
        "five.pinch_out" => "ピンチアウト",
        "five.swipe_up" => "上スワイプ",
        "five.swipe_down" => "下スワイプ",
        "press.light_threshold" => "軽い押し込みのしきい値",
        "press.deep_threshold" => "深い押し込みのしきい値",
        "press.haptic_strength" => "触覚の強さ",
        "press.haptic_ticks" => "押し込みで触覚を返す",
        "calibration" => "押し込みのキャリブレーション",
        "press.fingers" => "指ごとの押し込み操作",
        "long_hold.actions.one" => "1本指の長押し",
        "long_hold.actions.two" => "2本指の長押し",
        "long_hold.actions.three" => "3本指の長押し",
        "long_hold.actions.four" => "4本指の長押し",
        "long_hold.actions.five" => "5本指の長押し",
        "finger_hold.combos" | "thumb_hold.combos" => "組み合わせ",
        "corner.corners" => "コーナーの操作",
        "corner.size_mm" => "角の大きさ",
        "edge.left" => "左端",
        "edge.right" => "右端",
        "edge.top" => "上端",
        "edge.show_level" => "レベルを画面に表示する",
        "edge.haptic_ticks" => "段階ごとに触覚で知らせる",
        "window.flick_snap" => "フリックでスナップ",
        "window.corner_resize" => "対角を固定してサイズ変更",
        "ring.items" => "リングメニューの項目",
        "profiles" => "アプリ設定",
        other => {
            if let Some(l) = drag_row_label(other) {
                return l;
            }
            return other
                .strip_prefix("sensitivity.")
                .and_then(config::Sensitivity::label_of)
                .map_or_else(|| other.to_string(), str::to_string);
        }
    }
    .to_string()
}

/// `drag.three.enabled` and the like: the row's label from the diff's leaf path.
fn drag_row_label(path: &str) -> Option<String> {
    let rest = path.strip_prefix("drag.")?;
    let (row, leaf) = rest.split_once('.')?;
    let n = match row {
        "three" => 3,
        "four" => 4,
        "five" => 5,
        _ => return None,
    };
    let what = match leaf {
        "enabled" => "有効",
        "start" => "開始方法",
        "action" => "実行する動作",
        _ => return None,
    };
    Some(format!("{n}本指ドラッグ: {what}"))
}

const DETAILS_PER_SECTION: usize = 3;

/// Sections (in config order) whose values differ between `old` and `new`.
pub fn diff(old: &Config, new: &Config) -> Vec<Change> {
    let (Ok(toml::Value::Table(a)), Ok(toml::Value::Table(b))) =
        (toml::Value::try_from(old), toml::Value::try_from(new))
    else {
        return Vec::new();
    };
    let mut keys: Vec<&String> = a.keys().collect();
    keys.extend(b.keys().filter(|k| !a.contains_key(*k)));
    let mut out = Vec::new();
    for k in keys {
        let mut leaves = Vec::new();
        walk(k, a.get(k), b.get(k), &mut leaves);
        if leaves.is_empty() {
            continue;
        }
        let details = leaves
            .iter()
            .take(DETAILS_PER_SECTION)
            .map(|(l, d)| format!("{l}: {d}"))
            .collect();
        out.push(Change {
            section: section_label(k).to_string(),
            count: leaves.len(),
            details,
        });
    }
    out
}

pub fn status_from(state: PermissionState, driver_key_present: bool) -> HapticStatus {
    if state.is_enabled() {
        HapticStatus::Granted
    } else if state.usb == Some(false) || state.bluetooth == Some(false) || driver_key_present {
        // A present key with no value is the driver's default: administrators only.
        HapticStatus::NotGranted
    } else {
        HapticStatus::Unknown
    }
}

fn read_status() -> HapticStatus {
    status_from(permission::read_state(), permission::driver_key_present())
}

/// `YUBI_SHOT_SCROLL=<pixels>` scrolls every page while taking screenshots.
pub fn shot_scroll() -> Option<f32> {
    std::env::var_os("YUBI_SHOT")?;
    std::env::var("YUBI_SHOT_SCROLL").ok()?.parse().ok()
}

/// `YUBI_SHOT_PERM=granted|none|unknown` forces the row's state while taking screenshots.
fn shot_override() -> Option<HapticStatus> {
    std::env::var_os("YUBI_SHOT")?;
    match std::env::var("YUBI_SHOT_PERM").ok()?.as_str() {
        "granted" => Some(HapticStatus::Granted),
        "none" => Some(HapticStatus::NotGranted),
        "unknown" => Some(HapticStatus::Unknown),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Action;
    use std::fs;

    fn temp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("yubi-gio-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn diff_lists_changed_sections_only() {
        let a = Config::default();
        assert!(diff(&a, &a).is_empty());
        let mut b = a.clone();
        b.drag.only(4);
        b.five.swipe_up = Action::Keys("Win+Tab".into());
        let d = diff(&a, &b);
        let names: Vec<&str> = d.iter().map(|c| c.section.as_str()).collect();
        assert!(names.contains(&"ドラッグ") && names.contains(&"5本指ジェスチャー"));
        assert_eq!(d.len(), 2, "{d:?}");
        assert!(d[0].details[0].contains('→'));
    }

    #[test]
    fn a_real_drag_diff_shows_japanese_labels() {
        let a = Config::default();
        let mut b = a.clone();
        b.drag.three.enabled = !a.drag.three.enabled;
        b.drag.four.start = crate::config::DragStart::TapThenMove;
        b.drag.five.action = crate::config::DragAction::WindowResize;
        let d = diff(&a, &b);
        let text = d
            .iter()
            .flat_map(|c| c.details.iter())
            .cloned()
            .collect::<Vec<_>>()
            .join(
                "
",
            );
        assert!(text.contains("3本指ドラッグ: 有効"), "{text}");
        assert!(text.contains("4本指ドラッグ: 開始方法"), "{text}");
        assert!(text.contains("5本指ドラッグ: 実行する動作"), "{text}");
        assert!(!text.contains("drag."), "{text}");
    }

    #[test]
    fn autostart_is_not_imported() {
        let cur = Config::default();
        let mut imp = Imported {
            config: Config {
                general: crate::config::General {
                    start_with_windows: !cur.general.start_with_windows,
                    ..cur.general.clone()
                },
                ..Config::default()
            },
            warnings: Vec::new(),
        };
        let p = make_preview(&cur, PathBuf::new(), &mut imp);
        assert!(p.changes.is_empty());
        assert_eq!(
            p.imported.config.general.start_with_windows,
            cur.general.start_with_windows
        );
    }

    #[test]
    fn apply_backs_up_then_undo_restores() {
        let dir = temp_dir("apply");
        let path = dir.join("config.toml");
        let mut live = Config::default();
        live.drag.only(4);
        let mut new = Config::default();
        new.drag.only(3);
        new.sensitivity.hold_ms = 900;
        // What is on disk is what gets backed up (and the live config is not saved first).
        config::save(&path, &live).unwrap();
        let undo = apply_import(&mut live, &path, &new, false).unwrap();
        assert_eq!(live, new);
        assert!(undo.backup.as_ref().is_some_and(|b| b.exists()));
        assert_eq!(config::load(&path), new);
        undo_apply(&mut live, &path, &undo, false).unwrap();
        assert!(live.drag.on(4));
        assert!(config::load(&path).drag.on(4));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn undo_restores_a_backup_the_strict_import_would_refuse() {
        let dir = temp_dir("undo-tolerant");
        let path = dir.join("config.toml");
        // Loads fine (with warnings) but import_from would reject it.
        fs::write(
            &path,
            "schema_version = 2
[drag.five]
enabled = true
[four]
swipe_up = \"teleport\"
[[profiles]]
exe_name = \"a.exe\"
[[profiles]]
exe_name = \"A.EXE\"
",
        )
        .unwrap();
        assert!(config_io::import_from(&path).is_err());
        let mut live = config::load(&path);
        let undo = apply_import(&mut live, &path, &Config::default(), false).unwrap();
        assert!(!live.drag.any());
        undo_apply(&mut live, &path, &undo, false).unwrap();
        assert!(live.drag.on(5));
        assert_eq!(live.four.swipe_up, Action::None);
        assert_eq!(live.profiles.len(), 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn apply_backs_up_the_file_on_disk_not_the_live_config() {
        let dir = temp_dir("disk-backup");
        let path = dir.join("config.toml");
        let mut on_disk = Config::default();
        on_disk.drag.only(5);
        config::save(&path, &on_disk).unwrap();
        let mut live = Config::default();
        live.drag.only(4);
        let undo = apply_import(&mut live, &path, &Config::default(), false).unwrap();
        let b = undo.backup.unwrap();
        assert!(config::load(&b).drag.on(5));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_only_refuses_apply_and_undo_and_leaves_the_file() {
        let dir = temp_dir("readonly");
        let path = dir.join("config.toml");
        let future = "schema_version = 9
";
        fs::write(&path, future).unwrap();
        let mut live = Config::default();
        assert!(apply_import(&mut live, &path, &Config::default(), true).is_err());
        let undo = Undo {
            previous: Config::default(),
            backup: None,
        };
        assert!(undo_apply(&mut live, &path, &undo, true).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), future);
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1, "no backup made");
        let mut st = GeneralState::new(&egui::Context::default());
        st.read_only = true;
        let mut imp = Imported {
            config: Config::default(),
            warnings: Vec::new(),
        };
        st.preview = Some(make_preview(&live, path.clone(), &mut imp));
        st.apply_preview(&mut live, &path);
        assert!(st.preview.is_some());
        assert_eq!(fs::read_to_string(&path).unwrap(), future);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn apply_rebuilds_preview_when_live_config_changed() {
        let dir = temp_dir("stale");
        let path = dir.join("config.toml");
        let mut live = Config::default();
        let mut st = GeneralState::new(&egui::Context::default());
        let mut imp = Imported {
            config: Config {
                drag: crate::config::Drag {
                    four: crate::config::DragRow {
                        enabled: true,
                        ..Default::default()
                    },
                    ..Config::default().drag
                },
                ..Config::default()
            },
            warnings: Vec::new(),
        };
        st.preview = Some(make_preview(&live, path.clone(), &mut imp));
        live.general.start_with_windows = !live.general.start_with_windows;
        let before = live.clone();
        st.apply_preview(&mut live, &path);
        assert_eq!(live, before, "not applied");
        assert!(!path.exists());
        let p = st.preview.as_ref().expect("preview rebuilt");
        assert_eq!(
            p.imported.config.general.start_with_windows,
            live.general.start_with_windows
        );
        st.apply_preview(&mut live, &path);
        assert!(live.drag.on(4));
        assert_eq!(
            live.general.start_with_windows,
            before.general.start_with_windows
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn labels_are_japanese() {
        assert_eq!(setting_label("drag.four.start"), "4本指ドラッグ: 開始方法");
        assert_eq!(setting_label("x.unknown"), "x.unknown");
        assert_eq!(setting_label("sensitivity.hold_ms"), "長押しの時間");
        assert_eq!(setting_label("sensitivity.nope"), "sensitivity.nope");
    }

    #[test]
    fn status_mapping() {
        let s = |usb, bluetooth| PermissionState { usb, bluetooth };
        assert_eq!(
            status_from(s(Some(true), None), true),
            HapticStatus::Granted
        );
        assert_eq!(
            status_from(s(Some(false), None), false),
            HapticStatus::NotGranted
        );
        assert_eq!(status_from(s(None, None), true), HapticStatus::NotGranted);
        assert_eq!(status_from(s(None, None), false), HapticStatus::Unknown);
    }
}
