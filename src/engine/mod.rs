//! Gesture engine: a thread between the input thread and the UI.
//!
//! It receives every frame, runs the recognizer, owns the executor and keeps a coalesced
//! "latest state" for the UI (monitor, diagnostics). It also lets go of every held key and
//! button when the situation changes (see `Worker::release_everything`).

pub mod arbitration;
pub mod dispatch;
pub mod executor;
pub mod gestures;
pub mod pause;
pub mod practice;
pub mod recognizer;
pub mod replay;
pub mod resolve;
pub mod session;
pub mod tracking;
pub mod types;

#[cfg(test)]
mod gesture_tests;

use std::fs::File;
use std::io::BufWriter;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex, RwLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::input::{self, Contact, DeviceInfo, FpsCounter, InputEvent, InputHandle, TouchFrame};
use dispatch::Dispatcher;
use executor::Executor;
use pause::PauseCtl;
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
/// While the ring is open the loop wakes this often to look at the Esc key.
const RING_POLL: Duration = Duration::from_millis(30);

/// How often the foreground app is looked up while yubi runs.
const FOREGROUND_POLL: Duration = Duration::from_millis(500);

/// How often a refused button release is retried.
const RELEASE_RETRY: Duration = Duration::from_millis(100);
/// How long the engine keeps retrying a refused up when it shuts down.
const SHUTDOWN_RELEASE_WAIT: Duration = Duration::from_millis(1000);

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
    /// What the recognizer recognized lately and what became of it (practice page).
    pub practice: practice::History,
    /// yubi is paused (shortcut, tray, timer or a full-screen app): input passes through.
    pub paused: bool,
    /// What yubi last did for real (drag, window move, trigger), or why it did not.
    pub last_action: Option<String>,
    /// Executable name of the foreground window.
    pub foreground_exe: Option<String>,
    /// A per-app profile turns yubi off for the foreground app.
    pub app_blocked: bool,
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
    /// Practice mode: recognized gestures are shown, not run. The UI owns the switch.
    pub dry_run: Arc<AtomicBool>,
    /// The runtime pause switch (shared with the tray and the shortcut).
    pub pause: Arc<PauseCtl>,
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
        let dry_run = Arc::new(AtomicBool::new(false));
        let pause = Arc::new(PauseCtl::default());
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
            wake: wake.clone(),
            state: state.clone(),
            config: config.clone(),
            focused: focused.clone(),
            dry_run: dry_run.clone(),
            pause: pause.clone(),
            paused: false,
            practice: practice::Recorder::default(),
            counters: counters.clone(),
            stop: stop.clone(),
            interrupts,
            recognizer: Recognizer::default(),
            executor: Executor::new(Sink::default()),
            dispatch: Dispatcher::with_worker({
                let (state, wake) = (state.clone(), wake.clone());
                move |msg| {
                    if let Ok(mut s) = state.lock() {
                        s.last_action = Some(msg);
                    }
                    wake();
                }
            }),
            last_fg_poll: None,
            prev_contacts: 0,
            last_release_retry: None,
            haptics: None,
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
            dry_run,
            pause,
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
    dry_run: Arc<AtomicBool>,
    pause: Arc<PauseCtl>,
    /// The pause state the worker last acted on.
    paused: bool,
    practice: practice::Recorder,
    counters: Arc<input::InputCounters>,
    stop: Arc<AtomicBool>,
    interrupts: session::Interrupts,
    recognizer: Recognizer,
    executor: Executor<Sink>,
    dispatch: Dispatcher,
    /// When the foreground app was last looked up (it is polled, not hooked).
    last_fg_poll: Option<Instant>,
    /// Contacts in the previous frame (a touch begins when this is 0 and the next frame is not).
    prev_contacts: usize,
    last_release_retry: Option<Instant>,
    /// Started on the first press tick that needs it.
    haptics: Option<crate::haptic::Haptics>,
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
            self.sync_pause();
            let max_wait = if self.recognizer.ring_open() {
                RING_POLL
            } else {
                MAX_WAIT
            };
            let wait = self.recognizer.next_deadline().map_or(max_wait, |d| {
                Duration::from_micros(d.saturating_sub(input::now_us())).min(max_wait)
            });
            match self.rx.recv_timeout(wait) {
                Ok(ev) => self.handle(ev),
                Err(RecvTimeoutError::Timeout) => self.flush_recorder(),
                Err(RecvTimeoutError::Disconnected) => break,
            }
            if self.running() {
                let actions = self.recognizer.tick(input::now_us());
                self.apply(actions);
                // Esc closes the ring without running anything. The keystroke still reaches the
                // foreground app (the ring never takes the keyboard).
                if self.recognizer.ring_open() && dispatch::escape_down() {
                    let actions = self.recognizer.cancel_ring();
                    self.apply(actions);
                }
            }
            // The mouse hook's lease is short: keep a cursor freeze alive while it is wanted.
            self.dispatch.renew_suppression();
            self.poll_foreground(false);
            self.retry_release();
        }
        self.release_everything();
        // A refused up must not leave a button down system-wide after yubi exits: keep retrying
        // for a short, bounded time (the foreground may change back from an elevated window).
        let deadline = Instant::now() + SHUTDOWN_RELEASE_WAIT;
        while !self.executor.held().is_empty() && Instant::now() < deadline {
            std::thread::sleep(RELEASE_RETRY);
            self.executor.release_all();
        }
        self.flush_recorder();
    }

    /// Let go of every held key and button and forget any gesture in flight.
    fn release_everything(&mut self) {
        let note = self.dispatch.release_everything(&mut self.executor);
        self.recognizer.reset();
        self.with_state(|s| {
            s.finger_count = 0;
            if note.is_some() {
                s.last_action = note;
            }
        });
    }

    /// A refused up leaves a button owned by the executor with no gesture holding it; try again
    /// (at most every 100 ms) until it goes through.
    fn retry_release(&mut self) {
        if self.executor.held().is_empty()
            || self
                .last_release_retry
                .is_some_and(|t| t.elapsed() < RELEASE_RETRY)
        {
            return;
        }
        self.last_release_retry = Some(Instant::now());
        if let Some(note) = self.dispatch.retry_release(&mut self.executor) {
            self.with_state(|s| s.last_action = Some(note));
            (self.wake)();
        }
    }

    /// Pick up a changed config. A change in the middle of a gesture cancels it.
    fn sync_config(&mut self) {
        let generation = self.config.generation();
        if generation == self.seen_generation {
            return;
        }
        let first = self.seen_generation == u64::MAX;
        self.seen_generation = generation;
        self.recognizer.set_config_generation(generation);
        if !first && (self.recognizer.active() || !self.executor.held().is_empty()) {
            self.release_everything();
        }
        self.cfg = self.config.snapshot();
        // Start the haptics worker (it probes the driver) before the first tick needs it, but only
        // when press ticks can happen; with the press feature or its ticks off nothing is opened.
        if self.cfg.press.enabled && self.cfg.press.haptic_ticks && self.haptics.is_none() {
            self.haptics = Some(crate::haptic::Haptics::new());
        }
        if self.enabled && !self.cfg.gestures_enabled {
            self.release_everything();
        }
        self.enabled = self.cfg.gestures_enabled;
        let enabled = self.enabled;
        self.with_state(|s| s.gestures_enabled = enabled);
    }

    /// Gestures are on in the config and yubi is not paused.
    fn running(&self) -> bool {
        self.enabled && !self.paused
    }

    /// Follow the pause switch. Pausing in the middle of a gesture ends it cleanly: every held
    /// key and button is let go and the recognizer forgets the touch, so nothing stays down.
    fn sync_pause(&mut self) {
        self.pause.tick();
        let paused = self.pause.paused();
        if paused == self.paused {
            return;
        }
        self.paused = paused;
        if paused {
            self.release_everything();
        }
        self.with_state(|s| s.paused = paused);
        (self.wake)();
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

        let actions = if self.running() {
            // The app is decided once per touch session: look it up right before the first
            // contact of a session reaches the recognizer, which latches it.
            if touch_began(self.prev_contacts, active_contacts(&frame)) {
                self.poll_foreground(true);
            }
            self.recognizer.set_pad_dims(Some(f.device.size_mm));
            self.recognizer.step(&frame, &self.cfg)
        } else {
            Vec::new()
        };
        if self.log {
            eprintln!("yubi: frame {:?}", frame);
        }
        self.prev_contacts = active_contacts(&frame);
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

    /// Look up the foreground app (at most twice a second unless `force`) and decide whether a
    /// per-app profile switches yubi off for it.
    fn poll_foreground(&mut self, force: bool) {
        if !force
            && self
                .last_fg_poll
                .is_some_and(|t| t.elapsed() < FOREGROUND_POLL)
        {
            return;
        }
        self.last_fg_poll = Some(Instant::now());
        self.pause.set_fullscreen(
            self.cfg.general.pause_in_fullscreen && crate::os::window::fullscreen_exclusive(),
        );
        let exe = crate::os::window::foreground().and_then(crate::os::process::exe_name_of);
        let blocked = resolve::app_blocked(&self.cfg.profiles, exe.as_deref());
        self.dispatch.set_blocked(blocked);
        // A live gesture keeps the app it latched at touchdown; the next touch uses this one.
        self.recognizer.set_foreground(exe.as_deref(), blocked);
        let changed = self
            .state
            .lock()
            .is_ok_and(|s| s.foreground_exe != exe || s.app_blocked != blocked);
        if changed {
            self.with_state(|s| {
                s.foreground_exe = exe;
                s.app_blocked = blocked;
            });
            (self.wake)();
        }
    }

    fn apply(&mut self, actions: Vec<Action>) {
        for a in actions {
            if let Action::Debug(e) = &a {
                if self.log {
                    eprintln!("yubi: gesture {e:?}");
                }
                let (e, n) = (*e, self.recognizer.snapshot().finger_count);
                self.with_state(|s| {
                    s.last_event = Some(e);
                    s.finger_count = n;
                });
                (self.wake)();
            }
            let dry_run = self.dry_run.load(Ordering::Relaxed);
            self.dispatch.set_dry_run(dry_run);
            if let Action::PressTick { deep, provisional } = a {
                // Like every other output: no tick in a blocked app or in practice mode.
                if !dry_run && !self.dispatch.is_blocked() {
                    self.press_tick(deep, provisional);
                }
                continue;
            }
            let th = *self.recognizer.thresholds();
            // Practice mode and the history see the action first, in the mode it will run in.
            let mode = practice::Mode {
                dry_run,
                blocked: self.dispatch.is_blocked(),
            };
            if let Some(j) = self.practice.observe(&a, mode, Instant::now()) {
                self.with_state(|s| s.practice.push(j));
                (self.wake)();
            }
            let note =
                self.dispatch
                    .handle(&a, &mut self.executor, &th, crate::os::window::cursor_pos);
            if let Some(note) = note {
                self.with_state(|s| s.last_action = Some(note));
                (self.wake)();
            }
        }
        // The ring overlay could not be shown: close the ring rather than freeze blind.
        if self.dispatch.take_overlay_failed() {
            let actions = self.recognizer.cancel_ring();
            self.apply(actions);
        }
    }

    /// A haptic tick for a press (a provisional light press is weaker). Needs the permission and
    /// the driver; without them nothing happens.
    fn press_tick(&mut self, deep: bool, provisional: bool) {
        use crate::config::HapticStrength as S;
        use crate::haptic::{HapticStatus, Haptics, Strength};
        if !self.cfg.press.haptic_ticks {
            return;
        }
        let h = self.haptics.get_or_insert_with(Haptics::new);
        if h.status() != HapticStatus::Available {
            return;
        }
        let strength = if provisional {
            Strength::Weak
        } else {
            match (self.cfg.press.haptic_strength, deep) {
                (S::Light, false) => Strength::Weak,
                (S::Light, true) | (S::Medium, false) => Strength::Medium,
                (S::Medium, true) | (S::Firm, _) => Strength::Strong,
            }
        };
        h.actuate(strength);
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

/// The contacts the recognizer starts sessions from (the same predicate as its own filter): a
/// resting palm or a low-confidence contact does not count.
fn active_contacts(frame: &Frame) -> usize {
    frame
        .contacts
        .iter()
        .filter(|c| c.tip && c.confidence)
        .count()
}

/// True on the frame where a touch begins: the previous frame had no contact, this one has.
fn touch_began(prev: usize, now: usize) -> bool {
    prev == 0 && now > 0
}

#[cfg(test)]
mod tests {
    #[test]
    fn foreground_is_forced_only_when_a_touch_begins() {
        use super::touch_began;
        assert!(touch_began(0, 1));
        assert!(touch_began(0, 3));
        assert!(!touch_began(1, 1));
        assert!(!touch_began(2, 3));
        assert!(!touch_began(3, 0));
        assert!(!touch_began(0, 0));
    }

    #[test]
    fn a_resting_palm_does_not_hide_the_touch_begin() {
        use super::{Frame, active_contacts, touch_began};
        use crate::input::Contact;
        let c = |id, tip, confidence| Contact {
            id,
            x: 0.0,
            y: 0.0,
            x_mm: 0.0,
            y_mm: 0.0,
            tip,
            confidence,
            pressure: None,
        };
        let frame = |contacts| Frame {
            t_us: 0,
            button: false,
            contacts,
        };
        // A palm that is not a tip, and one the hardware is unsure of.
        let palm = frame(vec![c(1, false, true), c(2, true, false)]);
        assert_eq!(active_contacts(&palm), 0);
        // Fingers land while the palm rests: the raw count is already > 0, the active count is not.
        let touch = frame(vec![c(1, false, true), c(3, true, true), c(4, true, true)]);
        assert_eq!(touch.contacts.len(), 3);
        assert!(touch_began(active_contacts(&palm), active_contacts(&touch)));
        assert!(!touch_began(
            active_contacts(&touch),
            active_contacts(&touch)
        ));
    }

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
