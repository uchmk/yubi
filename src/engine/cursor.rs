//! Keeps Windows' touchpad cursor speed at what the config asks for, per foreground app.
//!
//! The speed is not a gesture: it is applied as soon as the foreground app (or the config)
//! changes, not per touch session. yubi only ever sets temporary values (no write to the user's
//! settings); the value Windows had before yubi touched it is remembered, written to a small
//! file next to the config while a temporary value is live, and put back on exit, when the
//! feature is turned off, or at the next start if yubi died without doing so.

use std::path::PathBuf;

use crate::os::touchpad;

/// What the OS side offers; a fake in tests.
pub trait Backend {
    fn get(&mut self) -> Option<u8>;
    /// Set a temporary value. False when Windows refused it.
    fn set(&mut self, speed: u8) -> bool;
}

/// The real touchpad.
pub struct Os;

impl Backend for Os {
    fn get(&mut self) -> Option<u8> {
        touchpad::cursor_speed()
    }
    fn set(&mut self, speed: u8) -> bool {
        touchpad::set_cursor_speed(speed, false)
    }
}

pub struct CursorSpeed<B: Backend> {
    backend: B,
    /// Where the pre-yubi value is kept while a temporary value is live.
    marker: Option<PathBuf>,
    /// The value Windows had before yubi changed it; `Some` while a temporary value is live.
    original: Option<u8>,
    /// The last value yubi set.
    applied: Option<u8>,
}

impl<B: Backend> CursorSpeed<B> {
    pub fn new(backend: B, marker: Option<PathBuf>) -> Self {
        Self {
            backend,
            marker,
            original: None,
            applied: None,
        }
    }

    /// At start: a marker file means an earlier yubi died with a temporary value live; put the
    /// user's own value back (Windows still holds the persisted one only in the file).
    pub fn recover(&mut self) {
        let Some(path) = self.marker.clone() else {
            return;
        };
        let Ok(text) = std::fs::read_to_string(&path) else {
            return;
        };
        if let Ok(v) = text.trim().parse::<u8>()
            && !self.backend.set(v)
        {
            return;
        }
        let _ = std::fs::remove_file(&path);
    }

    /// Make the speed `want` (`None` = hand it back to Windows). Writes only on a change.
    pub fn sync(&mut self, want: Option<u8>) {
        match want {
            Some(v) => {
                if self.applied == Some(v) {
                    return;
                }
                if self.original.is_none() {
                    let Some(cur) = self.backend.get() else {
                        return;
                    };
                    self.original = Some(cur);
                    if let Some(path) = &self.marker {
                        if let Some(dir) = path.parent() {
                            let _ = std::fs::create_dir_all(dir);
                        }
                        let _ = std::fs::write(path, cur.to_string());
                    }
                }
                if self.backend.set(v) {
                    self.applied = Some(v);
                }
            }
            None => self.restore(),
        }
    }

    /// Put the value from before yubi back and forget it.
    pub fn restore(&mut self) {
        let Some(orig) = self.original else {
            return;
        };
        if self.applied.is_some_and(|a| a != orig) && !self.backend.set(orig) {
            return;
        }
        self.original = None;
        self.applied = None;
        if let Some(path) = &self.marker {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake {
        value: u8,
        writes: Vec<u8>,
        refuse: bool,
    }

    impl Backend for &mut Fake {
        fn get(&mut self) -> Option<u8> {
            Some(self.value)
        }
        fn set(&mut self, speed: u8) -> bool {
            if self.refuse {
                return false;
            }
            self.value = speed;
            self.writes.push(speed);
            true
        }
    }

    fn fake(value: u8) -> Fake {
        Fake {
            value,
            writes: Vec::new(),
            refuse: false,
        }
    }

    #[test]
    fn writes_only_on_change_and_restores() {
        let mut f = fake(10);
        let mut c = CursorSpeed::new(&mut f, None);
        c.sync(Some(14));
        c.sync(Some(14));
        c.sync(Some(6));
        c.sync(Some(6));
        c.restore();
        c.restore();
        assert_eq!(f.writes, vec![14, 6, 10]);
        assert_eq!(f.value, 10);
    }

    #[test]
    fn turning_the_feature_off_restores_and_off_again_is_silent() {
        let mut f = fake(7);
        let mut c = CursorSpeed::new(&mut f, None);
        c.sync(None);
        c.sync(Some(3));
        c.sync(None);
        c.sync(None);
        // Back on later: the user's value is read again.
        f.value = 9;
        let mut c = CursorSpeed::new(&mut f, None);
        c.sync(Some(9));
        c.restore();
        assert_eq!(f.writes, vec![3, 7, 9]);
    }

    #[test]
    fn a_refused_write_is_retried_and_leaves_nothing_to_restore() {
        let mut f = fake(10);
        f.refuse = true;
        let mut c = CursorSpeed::new(&mut f, None);
        c.sync(Some(5));
        c.sync(Some(5));
        assert!(c.applied.is_none());
        c.restore();
        assert!(f.writes.is_empty());
    }

    #[test]
    fn a_marker_from_a_crashed_run_is_applied_and_removed() {
        let dir = std::env::temp_dir().join(format!("yubi-cursor-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let marker = dir.join("cursor-original.txt");
        let mut f = fake(10);
        {
            let mut c = CursorSpeed::new(&mut f, Some(marker.clone()));
            c.sync(Some(18));
            assert_eq!(std::fs::read_to_string(&marker).unwrap(), "10");
            // yubi dies here: no restore.
        }
        assert_eq!(f.value, 18);
        let mut c = CursorSpeed::new(&mut f, Some(marker.clone()));
        c.recover();
        assert_eq!(f.value, 10);
        assert!(!marker.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
