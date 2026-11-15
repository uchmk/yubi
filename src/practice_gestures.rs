//! Clean-up of the backup an older yubi left behind.
//!
//! Older builds wrote 0 into Windows' 3- and 4-finger gesture values while 練習モード was on and
//! put them back afterwards. That no longer happens (Windows does not apply the values live).
//! If such a build crashed, [`PRACTICE_BACKUP_FILE`] is still on disk: at start-up the values it
//! recorded are put back, but only those that still hold the 0 yubi wrote. A value the user
//! changed in the meantime is left alone. The file is deleted afterwards.

use std::io;
use std::path::Path;

use crate::win_gestures::{self, Backup, NAMES, RegOp, Values};

/// File next to the config that held the values taken before practice switched them off.
pub const PRACTICE_BACKUP_FILE: &str = "windows-gestures-practice-backup.toml";

/// Writes that undo yubi's old zeros: a value is restored only while it still equals 0 (what yubi
/// wrote). Present values are set back, missing ones are deleted again.
pub fn plan_leftover_restore(backup: &Backup, current: &Values) -> Vec<RegOp> {
    win_gestures::plan_restore(backup)
        .into_iter()
        .filter(|op| {
            let name = match op {
                RegOp::Set(n, _) | RegOp::Delete(n) => n,
            };
            NAMES
                .iter()
                .position(|n| n == name)
                .is_some_and(|i| current[i] == Some(0))
        })
        .collect()
}

/// Handle a leftover backup at start-up. Does nothing without the file. The file is deleted
/// whenever the registry was handled (even if nothing needed restoring); on a registry failure it
/// stays for the next start.
pub fn handle_leftover(backup_path: &Path) -> Result<(), String> {
    handle_leftover_with(
        backup_path,
        win_gestures::read_values,
        win_gestures::apply,
        |p| std::fs::remove_file(p),
    )
}

fn handle_leftover_with(
    backup_path: &Path,
    read: impl FnOnce() -> io::Result<Values>,
    write: impl FnOnce(&[RegOp]) -> io::Result<()>,
    remove: impl FnOnce(&Path) -> io::Result<()>,
) -> Result<(), String> {
    if !backup_path.exists() {
        return Ok(());
    }
    match win_gestures::read_backup(backup_path) {
        // A read (IO) failure says nothing about the content: keep the file and report it.
        Err(e) if e.kind() != io::ErrorKind::InvalidData && e.kind() != io::ErrorKind::Other => {
            return Err(format!("バックアップを読めません: {e}"));
        }
        Ok(backup) => {
            let current = read().map_err(|e| format!("レジストリを読めません: {e}"))?;
            write(&plan_leftover_restore(&backup, &current))
                .map_err(|e| format!("レジストリに書き込めません: {e}"))?;
        }
        // An unreadable backup can never be applied: drop it rather than retry it forever.
        Err(e) => eprintln!("yubi: ignoring unreadable practice backup: {e}"),
    }
    remove(backup_path).map_err(|e| format!("使用済みバックアップを削除できません: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn backup() -> Backup {
        // Slide3 had 1, tap3 was missing, slide4 had 1, tap4 was missing.
        win_gestures::backup_of(&[Some(1), None, Some(1), None])
    }

    #[test]
    fn restores_values_that_still_hold_yubis_zero() {
        let ops = plan_leftover_restore(&backup(), &[Some(0); 4]);
        assert_eq!(
            ops,
            vec![
                RegOp::Set("ThreeFingerSlideEnabled", 1),
                RegOp::Delete("ThreeFingerTapEnabled"),
                RegOp::Set("FourFingerSlideEnabled", 1),
                RegOp::Delete("FourFingerTapEnabled"),
            ]
        );
    }

    #[test]
    fn leaves_values_the_user_changed_since() {
        // Slide3 changed to 1 and tap3 deleted by the user; slide4 and tap4 still zero.
        let cur: Values = [Some(1), None, Some(0), Some(0)];
        assert_eq!(
            plan_leftover_restore(&backup(), &cur),
            vec![
                RegOp::Set("FourFingerSlideEnabled", 1),
                RegOp::Delete("FourFingerTapEnabled"),
            ]
        );
        assert!(plan_leftover_restore(&backup(), &[Some(1), None, Some(1), None]).is_empty());
    }

    fn dir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("yubi-pg-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn no_file_is_a_no_op() {
        let none = std::env::temp_dir().join("yubi-pg-none").join("none.toml");
        let r = handle_leftover_with(
            &none,
            || panic!("must not read"),
            |_| panic!("must not write"),
            |_| panic!("must not remove"),
        );
        assert!(r.is_ok());
    }

    #[test]
    fn file_is_deleted_after_restoring_only_untouched_values() {
        let d = dir("ok");
        let p = d.join(PRACTICE_BACKUP_FILE);
        win_gestures::write_backup_new(&p, &backup()).unwrap();
        let mut ops = Vec::new();
        handle_leftover_with(
            &p,
            || Ok([Some(0), Some(0), Some(7), None]),
            |o| {
                ops = o.to_vec();
                Ok(())
            },
            |p| std::fs::remove_file(p),
        )
        .unwrap();
        assert_eq!(ops.len(), 2);
        assert!(!p.exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_failed_write_keeps_the_file_for_the_next_start() {
        let d = dir("fail");
        let p = d.join(PRACTICE_BACKUP_FILE);
        win_gestures::write_backup_new(&p, &backup()).unwrap();
        let r = handle_leftover_with(
            &p,
            || Ok([Some(0); 4]),
            |_| Err(io::Error::other("locked")),
            |p| std::fs::remove_file(p),
        );
        assert!(r.is_err());
        assert!(p.exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn an_io_error_reading_keeps_the_file() {
        let d = dir("io");
        // A directory in place of the file: it exists but cannot be read as text.
        let p = d.join(PRACTICE_BACKUP_FILE);
        std::fs::create_dir_all(&p).unwrap();
        let r = handle_leftover_with(
            &p,
            || panic!("must not read the registry"),
            |_| panic!("must not write"),
            |_| panic!("must not remove"),
        );
        assert!(r.is_err());
        assert!(p.exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn an_unparsable_backup_is_deleted() {
        let d = dir("parse");
        let p = d.join(PRACTICE_BACKUP_FILE);
        std::fs::write(&p, "not = [valid").unwrap();
        let r = handle_leftover_with(
            &p,
            || panic!("must not read the registry"),
            |_| panic!("must not write"),
            |p| std::fs::remove_file(p),
        );
        assert!(r.is_ok());
        assert!(!p.exists());
        let _ = std::fs::remove_dir_all(&d);
    }
}
