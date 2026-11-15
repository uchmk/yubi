//! Resident behaviour: tray icon, close-to-tray, `--background`, autostart and single instance.

pub mod autostart;
mod single_instance;
mod tray;

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use eframe::egui;

/// What the tray menu asked for; the app applies it on its next frame.
#[derive(Debug, PartialEq, Eq)]
pub enum Event {
    ToggleGestures,
    Quit,
    /// Logoff or shutdown: save, stop the engine, then `ack_end_session`.
    EndSession,
}

/// `--background`: start with the window hidden (tray only). The value after `--record` is skipped.
pub fn is_background(args: &[OsString]) -> bool {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--record" {
            it.next();
        } else if a == "--background" {
            return true;
        }
    }
    false
}

/// Claim the single instance. `false` means another yubi runs: it was asked to show its window
/// (unless this launch is `--background`) and this process should exit.
pub fn enter(background: bool) -> bool {
    if single_instance::claim() {
        return true;
    }
    if !background {
        tray::signal_existing();
    }
    false
}

pub struct Resident {
    shared: Arc<tray::Shared>,
    rx: Receiver<Event>,
    thread: Option<JoinHandle<()>>,
    done: Receiver<()>,
    hint_marker: Option<PathBuf>,
}

impl Resident {
    /// Start the tray thread. `None` when the icon cannot be created (the app then behaves as a
    /// normal window: closing it exits).
    pub fn start(ctx: egui::Context, config_dir: Option<PathBuf>) -> Option<Self> {
        let (tx, rx) = mpsc::channel();
        let hint_marker = config_dir.map(|d| d.join("tray-hint-shown"));
        let shared = Arc::new(tray::Shared {
            ctx,
            tx,
            paused: Default::default(),
            hwnd: Default::default(),
            icon_ok: Default::default(),
            balloon: Mutex::new(None),
            hint_marker: hint_marker.clone(),
            end_ack: Default::default(),
        });
        tray::install(shared.clone());
        let (ready_tx, ready_rx) = mpsc::channel();
        let (done_tx, done) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("yubi-tray".into())
            .spawn(move || {
                tray::run(ready_tx);
                let _ = done_tx.send(());
            })
            .ok()?;
        if !ready_rx.recv().unwrap_or(false) {
            let _ = thread.join();
            return None;
        }
        Some(Self {
            shared,
            rx,
            thread: Some(thread),
            done,
            hint_marker,
        })
    }

    /// Is the notification-area icon in place (it may need retries while Explorer starts)?
    pub fn icon_ready(&self) -> bool {
        self.shared.icon_ok.load(Ordering::SeqCst)
    }

    pub fn poll(&self) -> Vec<Event> {
        self.rx.try_iter().collect()
    }

    /// Keep the tooltip and menu text in step with `gestures_enabled`.
    pub fn set_paused(&self, paused: bool) {
        if self.shared.paused.swap(paused, Ordering::Relaxed) != paused {
            tray::refresh();
        }
    }

    /// The window was just hidden: the first time ever, tell the user where yubi went.
    pub fn on_hidden(&self) {
        let Some(marker) = &self.hint_marker else {
            return;
        };
        // The tray thread writes the marker once the balloon was really shown.
        if marker.exists() {
            return;
        }
        if let Ok(mut b) = self.shared.balloon.lock() {
            *b = Some("yubi はタスクトレイで動作を続けます".to_string());
        }
        tray::balloon();
    }

    /// Remove the tray icon and stop its thread.
    pub fn shutdown(&mut self) {
        if let Some(t) = self.thread.take() {
            tray::close();
            // Bounded wait; if the tray thread hangs, detach it rather than hang the exit.
            if self.done.recv_timeout(Duration::from_secs(2)).is_ok() {
                let _ = t.join();
            }
        }
    }

    /// Tell the tray thread the end-of-session work is done.
    pub fn ack_end_session(&self) {
        tray::ack_end_session();
    }
}

impl Drop for Resident {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Vec<OsString> {
        v.iter().map(OsString::from).collect()
    }

    #[test]
    fn background_flag() {
        assert!(!is_background(&args(&[])));
        assert!(is_background(&args(&["--background"])));
        assert!(is_background(&args(&[
            "--record",
            "a.jsonl",
            "--background"
        ])));
        assert!(!is_background(&args(&["--record", "--background"])));
    }
}
