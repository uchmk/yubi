//! Gesture engine: a thread between the input thread and the UI.
//!
//! It receives every frame, runs the recognizer, owns the executor and keeps a coalesced
//! "latest state" for the UI (monitor, diagnostics). It also lets go of every held key and
//! button when the situation changes (see `Worker::release_everything`).

pub mod executor;
pub mod recognizer;
pub mod replay;
pub mod session;

use std::fs::File;
use std::io::BufWriter;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex, RwLock};
use std::thread::JoinHandle;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::input::{self, Contact, DeviceInfo, FpsCounter, InputEvent, InputHandle, TouchFrame};
use executor::Executor;
use recognizer::{Action, GestureEvent, Recognizer};

#[cfg(windows)]
type Sink = executor::SendInputSink;

/// Builds without Win32 have nothing to inject into.
#[cfg(not(windows))]
#[derive(Default)]
struct Sink;
#[cfg(not(windows))]
impl executor::InputSink for Sink {
    fn send(&mut self, _: executor::SynthEvent) -> bool {
        true
    }
}

/// Longest the engine sleeps, so stop requests and session events are noticed promptly.
const MAX_WAIT: Duration = Duration::from_millis(100);

/// What the recognizer sees: one decoded frame, independent of the device.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Frame {
    pub t_us: u64,
    #[serde(default)]
    pub button: bool,
    pub contacts: Vec<Contact>,
}

impl From<&TouchFrame> for Frame {
    fn from(f: &TouchFrame) -> Self {
        Self {
            t_us: f.t_us,
            button: f.button,
            contacts: f.contacts.clone(),
        }
    }
}

/// Config shared between the UI (writer) and the engine (reader) with a generation counter.
#[derive(Debug, Default)]
pub struct SharedConfig {
    cfg: RwLock<Config>,
    generation: AtomicU64,
}

impl SharedConfig {
    pub fn new(cfg: Config) -> Self {
        Self {
            cfg: RwLock::new(cfg),
            generation: AtomicU64::new(0),
        }
    }

    /// Store `cfg` if it differs from what is shared; bumps the generation then.
    pub fn publish(&self, cfg: &Config) -> bool {
        if self.cfg.read().is_ok_and(|c| *c == *cfg) {
            return false;
        }
        if let Ok(mut c) = self.cfg.write() {
            *c = cfg.clone();
            self.generation.fetch_add(1, Ordering::SeqCst);
            return true;
        }
        false
    }

    pub fn snapshot(&self) -> Config {
        self.cfg.read().map(|c| c.clone()).unwrap_or_default()
    }

    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }
}

/// Latest engine and input state for the UI. The engine overwrites it; the UI copies it out.
#[derive(Debug, Clone, Default)]
pub struct MonitorState {
    pub device: Option<Arc<DeviceInfo>>,
    pub frame: Option<TouchFrame>,
    pub frames_total: u64,
    /// Frames that arrived while the yubi window was not focused.
    pub frames_background: u64,
    pub partial_frames: u64,
    pub dropped_contacts: u64,
    /// Time since the previous frame.
    pub last_delta_ms: Option<f32>,
    pub fps: usize,
    /// `input::now_us` of the latest frame.
    pub last_frame_us: u64,
    pub peak_contacts: usize,
    pub finger_count: usize,
    pub last_event: Option<GestureEvent>,
    /// Master toggle as the engine sees it.
    pub gestures_enabled: bool,
    pub failed: Option<String>,
    /// The session lock / suspend watcher could not start or stopped.
    pub session_error: Option<String>,
    /// Problem with `--record`, if any.
    pub record_error: Option<String>,
}

pub struct Engine {
    pub state: Arc<Mutex<MonitorState>>,
    pub config: Arc<SharedConfig>,
    /// The UI sets this every frame from the window focus.
    pub focused: Arc<AtomicBool>,
    #[allow(dead_code)]
    pub counters: Arc<input::InputCounters>,
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
    input: InputHandle,
    session: session::SessionWatch,
}

impl Engine {
    /// Start the input thread, the session watcher and the engine thread.
    pub fn start(
        cfg: Config,
        wake: impl Fn() + Send + Sync + 'static,
        record: Option<PathBuf>,
    ) -> Self {
        let wake = Arc::new(wake);
        let mut input = input::spawn({
            let wake = wake.clone();
            move || wake()
        });
        let counters = input.counters.clone();
        let rx = input.take_rx().expect("fresh handle has a receiver");
        let config = Arc::new(SharedConfig::new(cfg));
        let state = Arc::new(Mutex::new(MonitorState::default()));
        let focused = Arc::new(AtomicBool::new(true));
        let stop = Arc::new(AtomicBool::new(false));
        let interrupts: session::Interrupts = Arc::default();
        let session = session::SessionWatch::start(interrupts.clone(), {
            let (state, wake) = (state.clone(), wake.clone());
            move |msg| {
                if let Ok(mut s) = state.lock() {
                    s.session_error = Some(msg);
                }
                wake();
            }
        });

        let worker = Worker {
            rx,
            wake,
            state: state.clone(),
            config: config.clone(),
            focused: focused.clone(),
            counters: counters.clone(),
            stop: stop.clone(),
            interrupts,
            recognizer: Recognizer::default(),
            executor: Executor::new(Sink::default()),
            cfg: Config::default(),
            seen_generation: u64::MAX,
            seen_interrupts: 0,
            enabled: false,
            fps: FpsCounter::default(),
            last_t_us: None,
            recorder: None,
            log: std::env::var_os("YUBI_LOG_INPUT").is_some(),
        };
        let join = std::thread::Builder::new()
            .name("yubi-engine".into())
            .spawn(move || worker.run(record))
            .ok();
        Self {
            state,
            config,
            focused,
            counters,
            stop,
            join,
            input,
            session,
        }
    }

    pub fn snapshot(&self) -> MonitorState {
        self.state.lock().map(|s| s.clone()).unwrap_or_default()
    }

    /// Stop everything. The engine releases held keys before it ends. Safe to call twice.
    pub fn shutdown(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
        self.input.stop();
        self.input.join();
        self.session.stop();
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.shutdown();
    }
}

struct Worker {
    rx: Receiver<InputEvent>,
    wake: Arc<dyn Fn() + Send + Sync>,
    state: Arc<Mutex<MonitorState>>,
    config: Arc<SharedConfig>,
    focused: Arc<AtomicBool>,
    counters: Arc<input::InputCounters>,
    stop: Arc<AtomicBool>,
    interrupts: session::Interrupts,
    recognizer: Recognizer,
    executor: Executor<Sink>,
    cfg: Config,
    seen_generation: u64,
    seen_interrupts: u64,
    enabled: bool,
    fps: FpsCounter,
    last_t_us: Option<u64>,
    recorder: Option<BufWriter<File>>,
    log: bool,
}

impl Worker {
    fn run(mut self, record: Option<PathBuf>) {
        if let Some(path) = record {
            match File::create(&path) {
                Ok(f) => self.recorder = Some(BufWriter::new(f)),
                Err(e) => {
                    let msg = format!("{}: {e}", path.display());
                    self.with_state(|s| s.record_error = Some(msg));
                }
            }
        }
        while !self.stop.load(Ordering::SeqCst) {
            self.sync_config();
            self.sync_interrupts();
            let wait = self.recognizer.next_deadline().map_or(MAX_WAIT, |d| {
                Duration::from_micros(d.saturating_sub(input::now_us())).min(MAX_WAIT)
            });
            match self.rx.recv_timeout(wait) {
                Ok(ev) => self.handle(ev),
                Err(RecvTimeoutError::Timeout) => self.flush_recorder(),
                Err(RecvTimeoutError::Disconnected) => break,
            }
            if self.enabled {
                let actions = self.recognizer.tick(input::now_us());
                self.apply(actions);
            }
        }
        self.release_everything();
        self.flush_recorder();
    }

    /// Let go of every held key and button and forget any gesture in flight.
    fn release_everything(&mut self) {
        self.executor.release_all();
        self.recognizer.reset();
        self.with_state(|s| s.finger_count = 0);
    }

    /// Pick up a changed config. A change in the middle of a gesture cancels it.
    fn sync_config(&mut self) {
        let generation = self.config.generation();
        if generation == self.seen_generation {
            return;
        }
        let first = self.seen_generation == u64::MAX;
        self.seen_generation = generation;
        if !first && (self.recognizer.active() || !self.executor.held().is_empty()) {
            self.release_everything();
        }
        self.cfg = self.config.snapshot();
        if self.enabled && !self.cfg.gestures_enabled {
            self.release_everything();
        }
        self.enabled = self.cfg.gestures_enabled;
        let enabled = self.enabled;
        self.with_state(|s| s.gestures_enabled = enabled);
    }

    fn sync_interrupts(&mut self) {
        let n = session::count(&self.interrupts);
        if n != self.seen_interrupts {
            self.seen_interrupts = n;
            self.release_everything();
        }
    }

    fn handle(&mut self, ev: InputEvent) {
        match ev {
            InputEvent::Frame(f) => self.on_frame(f),
            InputEvent::DeviceArrived(d) => self.with_state(|s| {
                s.device = Some(d);
                s.failed = None;
            }),
            InputEvent::DeviceRemoved(id) => {
                self.release_everything();
                self.with_state(|s| {
                    if s.device.as_ref().is_some_and(|d| d.id == id) {
                        s.device = None;
                        s.frame = None;
                    }
                });
            }
            InputEvent::Failed(msg) => {
                self.release_everything();
                self.with_state(|s| s.failed = Some(msg));
            }
        }
        (self.wake)();
    }

    fn on_frame(&mut self, f: TouchFrame) {
        let frame = Frame::from(&f);
        if let Some(w) = self.recorder.as_mut()
            && let Err(e) = replay::write_frame(w, &frame)
        {
            self.recorder = None;
            self.with_state(|s| s.record_error = Some(e.to_string()));
        }
        let delta = self
            .last_t_us
            .map(|p| f.t_us.saturating_sub(p) as f32 / 1000.0);
        self.last_t_us = Some(f.t_us);
        self.fps.record(f.t_us as f64 / 1e6);
        let fps = self.fps.rate(f.t_us as f64 / 1e6);
        let background = !self.focused.load(Ordering::Relaxed);
        let (partial, dropped) = (self.counters.partial_frames(), self.counters.contacts());

        let actions = if self.enabled {
            self.recognizer.step(&frame, &self.cfg)
        } else {
            Vec::new()
        };
        if self.log {
            eprintln!("yubi: frame {:?}", frame);
        }
        let snap = self.recognizer.snapshot();
        self.with_state(|s| {
            s.frames_total += 1;
            if background {
                s.frames_background += 1;
            }
            s.partial_frames = partial;
            s.dropped_contacts = dropped;
            s.last_delta_ms = delta;
            s.fps = fps;
            s.last_frame_us = f.t_us;
            s.peak_contacts = s.peak_contacts.max(f.contacts.len());
            s.finger_count = snap.finger_count;
            s.device = Some(f.device.clone());
            s.frame = Some(f);
        });
        self.apply(actions);
    }

    fn apply(&mut self, actions: Vec<Action>) {
        for a in actions {
            match a {
                Action::Debug(e) => {
                    if self.log {
                        eprintln!("yubi: gesture {e:?}");
                    }
                    let n = self.recognizer.snapshot().finger_count;
                    self.with_state(|s| {
                        s.last_event = Some(e);
                        s.finger_count = n;
                    });
                    (self.wake)();
                }
            }
        }
    }

    fn flush_recorder(&mut self) {
        if let Some(w) = self.recorder.as_mut() {
            let _ = std::io::Write::flush(w);
        }
    }

    fn with_state(&self, f: impl FnOnce(&mut MonitorState)) {
        if let Ok(mut s) = self.state.lock() {
            f(&mut s);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_config_bumps_generation_only_on_change() {
        let shared = SharedConfig::new(Config::default());
        let mut cfg = Config::default();
        assert!(!shared.publish(&cfg));
        assert_eq!(shared.generation(), 0);
        cfg.gestures_enabled = !cfg.gestures_enabled;
        assert!(shared.publish(&cfg));
        assert_eq!(shared.generation(), 1);
        assert_eq!(shared.snapshot(), cfg);
        assert!(!shared.publish(&cfg));
    }

    #[test]
    fn frame_converts_from_touch_frame() {
        let tf = TouchFrame {
            contacts: Vec::new(),
            button: true,
            scan_time: 9,
            t_us: 77,
            device: Arc::default(),
        };
        let f = Frame::from(&tf);
        assert_eq!((f.t_us, f.button), (77, true));
    }
}
