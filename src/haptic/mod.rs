//! Haptics client for the official Apple Magic Trackpad driver (v2.3).
//!
//! `Haptics` owns a worker thread that performs all I/O; the public methods only enqueue a command
//! or read a cached value, so they never block the caller for more than a lock acquisition.
//! Not yet wired into the app. Independent of `input`, `engine` and `app`.
#![allow(dead_code)]

pub mod permission;
mod scheduler;
pub mod transport;

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use scheduler::Scheduler;

/// Click strength. The driver value is 24 bits: byte 0 is the strength (0..=255), byte 1 is at most
/// 0x20 and byte 2 at most 0x40 (see [`Strength::value`], which clamps).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strength {
    Weak,
    Medium,
    Strong,
    Raw(u32),
}

impl Strength {
    pub const WEAK: u32 = 0x04_04_15;
    pub const MEDIUM: u32 = 0x06_06_17;
    pub const STRONG: u32 = 0x08_08_1E;

    /// The 24-bit driver value, with the upper two bytes clamped to what the driver accepts.
    pub fn value(self) -> u32 {
        let raw = match self {
            Strength::Weak => Self::WEAK,
            Strength::Medium => Self::MEDIUM,
            Strength::Strong => Self::STRONG,
            Strength::Raw(v) => v,
        };
        let b0 = raw & 0xFF;
        let b1 = ((raw >> 8) & 0xFF).min(0x20);
        let b2 = ((raw >> 16) & 0xFF).min(0x40);
        b0 | (b1 << 8) | (b2 << 16)
    }

    /// Parse `weak`, `medium`, `strong` or a number (`0x060617` / decimal).
    pub fn parse(s: &str) -> Option<Strength> {
        match s.trim().to_ascii_lowercase().as_str() {
            "weak" => Some(Strength::Weak),
            "medium" => Some(Strength::Medium),
            "strong" => Some(Strength::Strong),
            t => {
                let v = match t.strip_prefix("0x") {
                    Some(h) => u32::from_str_radix(h, 16).ok()?,
                    None => t.parse::<u32>().ok()?,
                };
                Some(Strength::Raw(v))
            }
        }
    }
}

/// Whether haptics can currently be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HapticStatus {
    Available,
    /// The endpoint exists but this (non-elevated) process may not open it. See [`permission`].
    AccessDenied,
    /// Driver endpoint not present (driver not installed / trackpad not connected). Also the
    /// value before the first probe finishes; see [`Haptics::is_ready`].
    NotFound,
    Error(String),
}

enum Cmd {
    Click(u32),
    Pulse {
        value: u32,
        count: u32,
        interval: Duration,
    },
    Refresh,
}

struct Shared {
    status: HapticStatus,
    battery: Option<u8>,
    ready: bool,
}

/// Capacity of the command queue. When the worker is stalled (e.g. a pipe call timing out) newer
/// commands are dropped instead of piling up; see [`Haptics::dropped`].
const QUEUE_CAPACITY: usize = 32;

/// Non-blocking haptics client.
pub struct Haptics {
    tx: SyncSender<Cmd>,
    dropped: AtomicU64,
    shared: Arc<Mutex<Shared>>,
    default_value: AtomicU32,
}

impl Haptics {
    /// Start the worker thread. It probes the endpoints immediately.
    pub fn new() -> Haptics {
        let (tx, rx) = mpsc::sync_channel::<Cmd>(QUEUE_CAPACITY);
        let shared = Arc::new(Mutex::new(Shared {
            status: HapticStatus::NotFound,
            battery: None,
            ready: false,
        }));
        let worker_shared = Arc::clone(&shared);
        std::thread::Builder::new()
            .name("yubi-haptics".into())
            .spawn(move || worker(rx, worker_shared))
            .expect("spawn haptics thread");
        Haptics {
            tx,
            dropped: AtomicU64::new(0),
            shared,
            default_value: AtomicU32::new(Strength::Medium.value()),
        }
    }

    /// Queue a command without blocking; drop it (and count it) when the queue is full.
    fn enqueue(&self, cmd: Cmd) {
        match self.tx.try_send(cmd) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
            }
            Err(TrySendError::Disconnected(_)) => {}
        }
    }

    /// Number of commands dropped because the queue was full.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// Enqueue one click. Returns immediately; bursts are coalesced and spaced >= 25 ms.
    pub fn actuate(&self, strength: Strength) {
        self.enqueue(Cmd::Click(strength.value()));
    }

    /// Strength used by [`Haptics::pulse`] (default Medium).
    pub fn set_default_strength(&self, strength: Strength) {
        self.default_value
            .store(strength.value(), Ordering::Relaxed);
    }

    /// Enqueue `count` clicks (clamped to 1..=16) `interval_ms` apart (minimum 25 ms).
    pub fn pulse(&self, count: u32, interval_ms: u32) {
        self.pulse_with(
            Strength::Raw(self.default_value.load(Ordering::Relaxed)),
            count,
            interval_ms,
        );
    }

    pub fn pulse_with(&self, strength: Strength, count: u32, interval_ms: u32) {
        self.enqueue(Cmd::Pulse {
            value: strength.value(),
            count,
            interval: Duration::from_millis(interval_ms as u64),
        });
    }

    /// Re-probe the endpoints and the battery (e.g. after a permission change). Asynchronous.
    pub fn refresh(&self) {
        self.enqueue(Cmd::Refresh);
    }

    /// Last known battery level (percent), if the driver reported one.
    pub fn battery(&self) -> Option<u8> {
        self.shared.lock().ok().and_then(|s| s.battery)
    }

    /// Last known status.
    pub fn status(&self) -> HapticStatus {
        self.shared
            .lock()
            .map(|s| s.status.clone())
            .unwrap_or_else(|_| HapticStatus::Error("lock poisoned".into()))
    }

    /// True once the first probe has finished.
    pub fn is_ready(&self) -> bool {
        self.shared.lock().map(|s| s.ready).unwrap_or(false)
    }

    /// Poll until the first probe finishes (for CLI tools; never call this from a UI thread).
    pub fn wait_ready(&self, timeout: Duration) -> bool {
        let end = Instant::now() + timeout;
        while Instant::now() < end {
            if self.is_ready() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        self.is_ready()
    }
}

impl Default for Haptics {
    fn default() -> Self {
        Self::new()
    }
}

fn worker(rx: mpsc::Receiver<Cmd>, shared: Arc<Mutex<Shared>>) {
    let mut backend = transport::Backend::new();
    let mut sched = Scheduler::new();
    let publish = |status: HapticStatus, battery: Option<Option<u8>>, ready: bool| {
        if let Ok(mut s) = shared.lock() {
            s.status = status;
            if let Some(b) = battery {
                s.battery = b;
            }
            s.ready |= ready;
        }
    };
    let (st, bat) = backend.probe();
    publish(st, Some(bat), true);

    loop {
        let now = Instant::now();
        let msg = match sched.next_due() {
            Some(due) => rx.recv_timeout(due.saturating_duration_since(now)),
            None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
        };
        match msg {
            Ok(Cmd::Click(v)) => sched.push_click(Instant::now(), v),
            Ok(Cmd::Pulse {
                value,
                count,
                interval,
            }) => sched.push_pulse(Instant::now(), value, count, interval),
            Ok(Cmd::Refresh) => {
                let (st, bat) = backend.probe();
                publish(st, Some(bat), true);
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        while let Some(v) = sched.pop_due(Instant::now()) {
            match backend.actuate(v) {
                Ok(_) => publish(HapticStatus::Available, None, false),
                Err(e) => publish(e.to_status(), None, false),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_match_control_panel() {
        assert_eq!(Strength::Weak.value(), 0x040415);
        assert_eq!(Strength::Medium.value(), 0x060617);
        assert_eq!(Strength::Strong.value(), 0x08081E);
    }

    #[test]
    fn raw_values_are_clamped() {
        assert_eq!(Strength::Raw(0xFFFFFFFF).value(), 0x4020FF);
        assert_eq!(Strength::Raw(0x000010).value(), 0x000010);
    }

    #[test]
    fn full_queue_drops_and_counts_without_blocking() {
        let (tx, _rx) = mpsc::sync_channel::<Cmd>(QUEUE_CAPACITY);
        let h = Haptics {
            tx,
            dropped: AtomicU64::new(0),
            shared: Arc::new(Mutex::new(Shared {
                status: HapticStatus::NotFound,
                battery: None,
                ready: false,
            })),
            default_value: AtomicU32::new(0),
        };
        let start = Instant::now();
        for _ in 0..QUEUE_CAPACITY + 10 {
            h.actuate(Strength::Medium);
        }
        assert_eq!(h.dropped(), 10);
        assert!(start.elapsed() < Duration::from_millis(50));
    }

    #[test]
    fn strength_parse() {
        assert_eq!(Strength::parse("Weak"), Some(Strength::Weak));
        assert_eq!(Strength::parse("0x060617"), Some(Strength::Raw(0x060617)));
        assert_eq!(Strength::parse("21"), Some(Strength::Raw(21)));
        assert_eq!(Strength::parse("loud"), None);
    }
}
