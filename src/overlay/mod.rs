//! Ring-menu overlay window (it also shows the small volume / brightness level HUD).
//!
//! The engine drives it with messages only (`OverlayCmd`); the overlay has no gesture logic. It
//! owns one thread with its own message loop and one click-through, never-activating, topmost
//! layered window that is drawn from a premultiplied 32-bit DIB (see `paint.rs`, `window.rs`).
//!
//! Threading: `Overlay::send` is callable from any thread. Commands go through a bounded channel
//! and a posted wake-up message; `send` never blocks (a full queue returns an error and the
//! command is dropped). Stale generations are ignored by `model::Model`. `state()` tells the
//! engine whether the window is usable, so it can cancel the ring when it is `Failed`.
//!
//! This module is self-contained (no `crate::` paths) so `src/bin/overlay-demo.rs` can include it.
#![allow(dead_code)]

mod geometry;
mod level;
mod model;
mod paint;
#[cfg(windows)]
mod window;

#[allow(unused_imports)]
pub use level::{LevelInfo, LevelKind};
#[allow(unused_imports)]
pub use model::{OverlayCmd, RingImage, RingSlot, RingStyle};

/// Render a ring to memory (premultiplied pixels) for the screenshot test.
#[cfg(all(test, windows))]
pub fn snapshot(
    items: Vec<RingSlot>,
    highlight: Option<usize>,
    centre: bool,
    dpi: u32,
) -> Result<(i32, Vec<u32>), String> {
    let hl = match (highlight, centre) {
        (Some(i), _) => paint::Highlight::Selected(i),
        (None, true) => paint::Highlight::Centre,
        (None, false) => paint::Highlight::None,
    };
    window::snapshot(items, hl, dpi)
}

use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OverlayState {
    Hidden,
    Shown,
    Failed(String),
}

/// State shared with the window thread.
pub(crate) struct Shared {
    pub state: Mutex<OverlayState>,
    /// The window handle as an integer (0 until created / after destruction).
    pub hwnd: AtomicIsize,
}

impl Shared {
    pub(crate) fn set_state(&self, s: OverlayState) {
        if let Ok(mut g) = self.state.lock() {
            // A failure is sticky.
            if !matches!(*g, OverlayState::Failed(_)) {
                *g = s;
            }
        }
    }
}

pub struct Overlay {
    tx: SyncSender<OverlayCmd>,
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
    /// Signalled when the window thread has finished (for a bounded `stop`).
    done: mpsc::Receiver<()>,
}

impl Overlay {
    /// Create the window on its own thread. Fails if the window cannot be created.
    #[cfg(windows)]
    pub fn start() -> Result<Overlay, String> {
        let (tx, rx) = mpsc::sync_channel::<OverlayCmd>(64);
        let shared = Arc::new(Shared {
            state: Mutex::new(OverlayState::Hidden),
            hwnd: AtomicIsize::new(0),
        });
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();
        let sh = shared.clone();
        let (done_tx, done) = mpsc::channel::<()>();
        let thread = std::thread::Builder::new()
            .name("yubi-overlay".into())
            .spawn(move || {
                window::run(rx, sh, ready_tx);
                let _ = done_tx.send(());
            })
            .map_err(|e| format!("overlay thread: {e}"))?;
        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Overlay {
                tx,
                shared,
                done,
                thread: Some(thread),
            }),
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(e)
            }
            Err(_) => Err("overlay thread ended before it was ready".into()),
        }
    }

    /// Queue a command. Returns an error (and drops the command) if the queue is full or the
    /// overlay has stopped.
    pub fn send(&self, cmd: OverlayCmd) -> Result<(), String> {
        self.tx.try_send(cmd).map_err(|e| e.to_string())?;
        if self.wake() {
            Ok(())
        } else {
            Err("overlay window is gone".into())
        }
    }

    pub fn state(&self) -> OverlayState {
        self.shared
            .state
            .lock()
            .map(|g| g.clone())
            .unwrap_or_else(|_| OverlayState::Failed("poisoned".into()))
    }

    /// Stop the thread and destroy the window. Also done on drop. Waits at most one second; a
    /// thread that does not finish by then is detached.
    pub fn stop(&mut self) {
        #[cfg(windows)]
        window::post_stop(self.shared.hwnd.load(Ordering::SeqCst));
        // If the thread does not finish in time, dropping the handle detaches it.
        if let Some(t) = self.thread.take()
            && self.done.recv_timeout(Duration::from_secs(1)).is_ok()
        {
            let _ = t.join();
        }
    }

    fn wake(&self) -> bool {
        #[cfg(windows)]
        return window::post_wake(self.shared.hwnd.load(Ordering::SeqCst));
        #[cfg(not(windows))]
        false
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        self.stop();
    }
}
