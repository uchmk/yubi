//! Settings export / import core (no UI). Pure file and TOML handling on top of `config`.
//!
//! Flow for an import: `import_from(path)` -> show warnings -> `backup_current(dir)` ->
//! `config::save` the imported config. Validation reuses `Config::normalize`: any value it changes
//! is reported as a warning (Japanese, for the UI). Unknown keys are ignored exactly like
//! `config::load` does (no `deny_unknown_fields`).
#![allow(dead_code)]

use std::fmt;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::config::{self, Config};

/// Imports larger than this are rejected before parsing.
pub const MAX_IMPORT_BYTES: u64 = 1024 * 1024;

#[derive(Debug)]
pub enum ConfigIoError {
    Io(io::Error),
    /// The file is larger than [`MAX_IMPORT_BYTES`].
    TooLarge(u64),
    /// The file is not valid TOML / not a yubi config (message from the parser).
    Parse(String),
    /// The config could not be serialized.
    Serialize(String),
    /// The file was written by a newer yubi (its `schema_version`).
    FutureVersion(u32),
    /// An action or edge token this build does not know.
    UnknownToken(String),
    /// The same exe name appears in more than one profile.
    DuplicateProfile(String),
    /// A per-app setting for a gesture that has no global setting (`exe: id`).
    UnbindableOverride(String),
    /// A per-app setting for a gesture the engine does not resolve yet (`exe: id`).
    InertOverride(String),
    /// A per-app setting whose key is no gesture id or edge key (`exe: key`).
    BadOverrideKey(String),
}

impl fmt::Display for ConfigIoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "ファイルを読み書きできません: {e}"),
            Self::TooLarge(n) => write!(
                f,
                "ファイルが大きすぎます ({n} バイト。上限は {MAX_IMPORT_BYTES} バイト)"
            ),
            Self::Parse(m) => write!(f, "設定ファイルを読み取れません: {m}"),
            Self::Serialize(m) => write!(f, "設定を書き出せません: {m}"),
            Self::FutureVersion(v) => write!(
                f,
                "この設定ファイルは新しい版の yubi で作られています (schema_version = {v})。読み込めません"
            ),
            Self::UnknownToken(t) => write!(
                f,
                "この版の yubi では読み取れない動作「{t}」が含まれています。読み込みを中止しました"
            ),
            Self::DuplicateProfile(e) => write!(
                f,
                "アプリ別の設定に「{e}」が重複しています。読み込みを中止しました"
            ),
            Self::UnbindableOverride(k) => write!(
                f,
                "アプリ別の設定「{k}」は割り当てられないジェスチャーです。読み込みを中止しました"
            ),
            Self::InertOverride(k) => write!(
                f,
                "アプリ別の設定「{k}」のコーナー・ホールドは、まだ適用されません。読み込みを中止しました"
            ),
            Self::BadOverrideKey(k) => write!(
                f,
                "アプリ別の設定「{k}」はジェスチャー名として読み取れません。読み込みを中止しました"
            ),
        }
    }
}

impl std::error::Error for ConfigIoError {}

impl From<io::Error> for ConfigIoError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

pub type Result<T> = std::result::Result<T, ConfigIoError>;

/// An imported config (already normalized) and what had to be corrected.
#[derive(Debug, Clone, PartialEq)]
pub struct Imported {
    pub config: Config,
    /// Human-readable Japanese notes, one per value that was out of range and clamped.
    pub warnings: Vec<String>,
}

pub(crate) fn unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// `YYYY-MM-DD` (UTC) for a unix timestamp (Howard Hinnant's civil-from-days).
pub(crate) fn date_utc(secs: u64) -> String {
    let z = (secs / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

fn header(secs: u64) -> String {
    format!(
        "# yubi settings export {} {}\n",
        env!("CARGO_PKG_VERSION"),
        date_utc(secs)
    )
}

/// Atomic write (temp file next to the target, flush, rename).
fn write_atomic(path: &Path, text: &str) -> io::Result<()> {
    if let Some(dir) = path.parent()
        && !dir.as_os_str().is_empty()
    {
        fs::create_dir_all(dir)?;
    }
    let mut tmp_name = path.file_name().unwrap_or_default().to_os_string();
    tmp_name.push(".tmp");
    let tmp = path.with_file_name(tmp_name);
    let write = || -> io::Result<()> {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
        drop(f);
        fs::rename(&tmp, path)
    };
    write().inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })
}

/// Write `cfg` as TOML (same serializer as `config::save`) with a version/date header comment.
/// The pressure calibration belongs to one trackpad, so it is not exported.
pub fn export_to(path: &Path, cfg: &Config) -> Result<()> {
    let mut shared = cfg.clone();
    shared.calibration.clear();
    let body =
        toml::to_string_pretty(&shared).map_err(|e| ConfigIoError::Serialize(e.to_string()))?;
    write_atomic(path, &format!("{}{body}", header(unix_secs())))?;
    Ok(())
}

/// Parse a config file for import. Rejects files over [`MAX_IMPORT_BYTES`].
pub fn import_from(path: &Path) -> Result<Imported> {
    parse_import(&read_limited(path)?)
}

/// Read a backup the app wrote itself (undo). It goes through the same tolerant parse as the normal
/// load: a file the app started with must be restorable, so unknown tokens, duplicate profiles and
/// bad overrides are only reported in `warnings`. A newer `schema_version` is still refused.
pub fn restore_from(path: &Path) -> Result<Imported> {
    let parsed = parse_tolerant(&read_limited(path)?)?;
    Ok(Imported {
        config: parsed.config,
        warnings: parsed.warnings,
    })
}

fn read_limited(path: &Path) -> Result<String> {
    let len = fs::metadata(path)?.len();
    if len > MAX_IMPORT_BYTES {
        return Err(ConfigIoError::TooLarge(len));
    }
    let mut buf = Vec::new();
    fs::File::open(path)?
        .take(MAX_IMPORT_BYTES + 1)
        .read_to_end(&mut buf)?;
    if buf.len() as u64 > MAX_IMPORT_BYTES {
        return Err(ConfigIoError::TooLarge(buf.len() as u64));
    }
    String::from_utf8(buf).map_err(|e| ConfigIoError::Parse(e.to_string()))
}

fn parse_tolerant(text: &str) -> Result<config::Parsed> {
    config::parse_text(text).map_err(|e| match e {
        config::ParseError::Syntax(m) => ConfigIoError::Parse(m),
        config::ParseError::FutureVersion(v) => ConfigIoError::FutureVersion(v),
    })
}

fn parse_import(text: &str) -> Result<Imported> {
    let parsed = parse_tolerant(text)?;
    // An import is a deliberate act: a token this build cannot read, or a profile that appears
    // twice, would silently change what the user meant, so the file is refused (load only warns).
    if let Some(token) = parsed.unknown_tokens.first() {
        return Err(ConfigIoError::UnknownToken(token.clone()));
    }
    if let Some(exe) = parsed.duplicate_profiles.first() {
        return Err(ConfigIoError::DuplicateProfile(exe.clone()));
    }
    if let Some(k) = parsed.unbindable_overrides.first() {
        return Err(ConfigIoError::UnbindableOverride(k.clone()));
    }
    if let Some(k) = parsed.inert_overrides.first() {
        return Err(ConfigIoError::InertOverride(k.clone()));
    }
    if let Some(k) = parsed.bad_override_keys.first() {
        return Err(ConfigIoError::BadOverrideKey(k.clone()));
    }
    Ok(Imported {
        config: parsed.config,
        warnings: parsed.warnings,
    })
}

/// Copy `<dir>/config.toml` to `<dir>/config.backup-<unix-seconds>.toml` (never overwriting an
/// older backup made in the same second). Returns the backup path, or `None` when there is no
/// `config.toml` yet (fresh install: nothing to back up).
pub fn backup_current(dir: &Path) -> Result<Option<PathBuf>> {
    let src = dir.join("config.toml");
    if !src.exists() {
        return Ok(None);
    }
    let secs = unix_secs();
    let mut n = 0u32;
    let dst = loop {
        let name = if n == 0 {
            format!("config.backup-{secs}.toml")
        } else {
            format!("config.backup-{secs}-{n}.toml")
        };
        let candidate = dir.join(name);
        if !candidate.exists() {
            break candidate;
        }
        n += 1;
    };
    fs::copy(&src, &dst)?;
    Ok(Some(dst))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Action;

    fn temp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("yubi-cio-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn date_formatting() {
        assert_eq!(date_utc(0), "1970-01-01");
        assert_eq!(date_utc(951_782_400), "2000-02-29");
        assert_eq!(date_utc(1_700_000_000), "2023-11-14");
    }

    #[test]
    fn round_trip() {
        let dir = temp_dir("rt");
        let path = dir.join("export.toml");
        let mut cfg = Config {
            gestures_enabled: false,
            ..Config::default()
        };
        cfg.five.swipe_up = Action::Keys("Win+Tab".into());
        cfg.drag.fingers = 4;
        export_to(&path, &cfg).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("# yubi settings export "));
        let got = import_from(&path).unwrap();
        assert_eq!(got.config, cfg);
        assert!(got.warnings.is_empty());
        assert!(!dir.join("export.toml.tmp").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn export_leaves_out_the_device_calibration() {
        let dir = temp_dir("cal");
        let path = dir.join("export.toml");
        let mut cfg = Config::default();
        cfg.calibration.insert(
            "05ac:0324".into(),
            crate::config::Calibration {
                baseline: 0.02,
                f1: 0.4,
                ..Default::default()
            },
        );
        export_to(&path, &cfg).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(!text.contains("05ac"), "calibration leaked: {text}");
        assert!(import_from(&path).unwrap().config.calibration.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn too_large_is_rejected() {
        let dir = temp_dir("big");
        let path = dir.join("big.toml");
        fs::write(&path, vec![b'#'; MAX_IMPORT_BYTES as usize + 1]).unwrap();
        assert!(matches!(
            import_from(&path),
            Err(ConfigIoError::TooLarge(n)) if n == MAX_IMPORT_BYTES + 1
        ));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn broken_toml_has_message() {
        let dir = temp_dir("bad");
        let path = dir.join("bad.toml");
        fs::write(&path, "gestures_enabled = [oops").unwrap();
        let err = import_from(&path).unwrap_err();
        assert!(matches!(err, ConfigIoError::Parse(_)));
        assert!(err.to_string().starts_with("設定ファイルを読み取れません"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn out_of_range_warns_and_unknown_keys_pass() {
        let got = parse_import(
            "future_key = 1\n[long_hold]\nhold_ms = 60000\nfingers = 0\n[drag]\nfingers = 9\n",
        )
        .unwrap();
        assert_eq!(got.config.sensitivity.hold_ms, 1500);
        assert_eq!(got.config.drag.fingers, 5);
        assert_eq!(got.warnings.len(), 3, "{:?}", got.warnings);
        assert!(got.warnings.iter().any(|m| m.contains("60000")));
    }

    #[test]
    fn import_rejects_unknown_tokens_future_versions_and_duplicate_profiles() {
        let err = parse_import(
            "schema_version = 2
[four]
swipe_up = \"teleport\"
",
        )
        .unwrap_err();
        assert!(matches!(&err, ConfigIoError::UnknownToken(t) if t == "teleport"));
        assert!(err.to_string().contains("teleport"));
        let err = parse_import(
            "schema_version = 2
[edge]
left = \"warp\"
",
        )
        .unwrap_err();
        assert!(err.to_string().contains("warp"));
        let err = parse_import(
            "schema_version = 3
",
        )
        .unwrap_err();
        assert!(matches!(err, ConfigIoError::FutureVersion(3)));
        assert!(err.to_string().contains("schema_version = 3"));
        let err = parse_import(
            "schema_version = 2
[[profiles]]
exe_name = \"a.exe\"
[[profiles]]
exe_name = \"A.EXE\"
",
        )
        .unwrap_err();
        assert!(matches!(&err, ConfigIoError::DuplicateProfile(e) if e == "A.EXE"));
        let err = parse_import(
            "schema_version = 2
[[profiles]]
exe_name = \"a.exe\"
[profiles.overrides]
\"swipe.3.left\" = \"none\"
",
        )
        .unwrap_err();
        assert!(matches!(&err, ConfigIoError::UnbindableOverride(k) if k.contains("swipe.3.left")));
        let err = parse_import(
            "schema_version = 2
[[profiles]]
exe_name = \"a.exe\"
[profiles.overrides]
\"corner.br.swipe\" = \"none\"
",
        )
        .unwrap_err();
        assert!(matches!(&err, ConfigIoError::InertOverride(k) if k.contains("corner.br.swipe")));
        let err = parse_import(
            "schema_version = 2
[[profiles]]
exe_name = \"a.exe\"
[profiles.overrides]
\"hold.thumb.index.swipe\" = \"none\"
",
        )
        .unwrap_err();
        assert!(matches!(err, ConfigIoError::InertOverride(_)));
        let err = parse_import(
            "schema_version = 2
[[profiles]]
exe_name = \"a.exe\"
[profiles.overrides]
\"nonsense\" = \"none\"
[profiles.edge_overrides]
\"edge.bottom\" = \"none\"
",
        )
        .unwrap_err();
        assert!(matches!(&err, ConfigIoError::BadOverrideKey(k) if k.contains("nonsense")));
        // Legacy tokens and brush_size are not errors: they load as none and are reported.
        let got = parse_import(
            "schema_version = 2
[four]
tap = \"volume\"
[edge]
left = \"brush_size\"
",
        )
        .unwrap();
        assert_eq!(got.config.four.tap, crate::config::Action::None);
        assert!(got.warnings.len() >= 2, "{:?}", got.warnings);
        // An old (schema 1) export imports through the migration.
        let got = parse_import(
            "[long_hold]
hold_ms = 700
",
        )
        .unwrap();
        assert_eq!(got.config.sensitivity.hold_ms, 700);
        assert_eq!(got.config.schema_version, 2);
    }

    #[test]
    fn restore_is_tolerant_but_refuses_future_versions() {
        let dir = temp_dir("restore");
        let path = dir.join("backup.toml");
        fs::write(
            &path,
            "schema_version = 2
[four]
swipe_up = \"teleport\"
[[profiles]]
exe_name = \"a.exe\"
[profiles.overrides]
\"corner.br.swipe\" = \"none\"
[[profiles]]
exe_name = \"A.EXE\"
",
        )
        .unwrap();
        assert!(import_from(&path).is_err());
        let got = restore_from(&path).unwrap();
        assert_eq!(got.config.four.swipe_up, crate::config::Action::None);
        assert_eq!(got.config.profiles.len(), 1);
        assert!(!got.warnings.is_empty());
        fs::write(
            &path,
            "schema_version = 9
",
        )
        .unwrap();
        assert!(matches!(
            restore_from(&path),
            Err(ConfigIoError::FutureVersion(9))
        ));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_file_is_io_error() {
        let dir = temp_dir("missing");
        assert!(matches!(
            import_from(&dir.join("nope.toml")),
            Err(ConfigIoError::Io(_))
        ));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn backup_naming_and_no_overwrite() {
        let dir = temp_dir("bak");
        fs::write(dir.join("config.toml"), "gestures_enabled = true\n").unwrap();
        let a = backup_current(&dir).unwrap().unwrap();
        let b = backup_current(&dir).unwrap().unwrap();
        assert_ne!(a, b);
        for p in [&a, &b] {
            let name = p.file_name().unwrap().to_string_lossy().into_owned();
            assert!(
                name.starts_with("config.backup-") && name.ends_with(".toml"),
                "{name}"
            );
            assert_eq!(fs::read_to_string(p).unwrap(), "gestures_enabled = true\n");
        }
        fs::remove_file(dir.join("config.toml")).unwrap();
        assert!(backup_current(&dir).unwrap().is_none());
        let _ = fs::remove_dir_all(&dir);
    }
}
