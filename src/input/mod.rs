//! Trackpad input: platform-independent types and pure helpers.
//! The Windows Raw Input reader lives in `rawinput` and feeds `InputEvent`s
//! over a channel; nothing here depends on the UI.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, OnceLock};
use std::thread::JoinHandle;
use std::time::Instant;

use serde::{Deserialize, Serialize};

#[cfg(windows)]
mod rawinput;

/// Static description of a touch pad as reported by its HID descriptor.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DeviceInfo {
    /// Stable id: the device interface path.
    pub id: String,
    pub name: String,
    /// Logical (min, max) of X and Y.
    pub x_range: (i32, i32),
    pub y_range: (i32, i32),
    /// Logical (min, max) of tip pressure, if the pad reports it.
    pub pressure_range: Option<(i32, i32)>,
    /// Number of finger slots per report.
    pub finger_slots: usize,
    /// Physical size of the sensing area in millimetres (width, height).
    pub size_mm: (f32, f32),
}

/// Sensing area of the Magic Trackpad 2 (PID 0324), used when the descriptor has no usable units.
pub const DEFAULT_SIZE_MM: (f32, f32) = (160.0, 114.9);

/// Physical length of a HID axis in millimetres from its physical range and units.
/// Only length units (SI linear = cm, English linear = inch) are understood.
pub fn extent_mm(phys_min: i32, phys_max: i32, units: u32, units_exp: u32) -> Option<f32> {
    let span = i64::from(phys_max) - i64::from(phys_min);
    if span <= 0 {
        return None;
    }
    let system = units & 0xF;
    let length_exp = (units >> 4) & 0xF;
    if length_exp != 1 || (units >> 8) != 0 {
        return None;
    }
    let exp = (units_exp & 0xF) as i32;
    let exp = if exp >= 8 { exp - 16 } else { exp };
    let unit_mm = match system {
        1 => 10.0,
        3 => 25.4,
        _ => return None,
    };
    let mm = span as f32 * 10f32.powi(exp) * unit_mm;
    (20.0..=1000.0).contains(&mm).then_some(mm)
}

impl DeviceInfo {
    /// Width / height of the sensing area, falling back to 16:10.
    pub fn aspect(&self) -> f32 {
        let w = (i64::from(self.x_range.1) - i64::from(self.x_range.0)) as f32;
        let h = (i64::from(self.y_range.1) - i64::from(self.y_range.0)) as f32;
        if w > 0.0 && h > 0.0 { w / h } else { 1.6 }
    }
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Contact {
    pub id: u32,
    /// Normalized 0..1 (left to right).
    #[serde(default)]
    pub x: f32,
    /// Normalized 0..1 (top to bottom).
    #[serde(default)]
    pub y: f32,
    /// Position in millimetres from the top-left corner.
    #[serde(default)]
    pub x_mm: f32,
    #[serde(default)]
    pub y_mm: f32,
    #[serde(default = "yes")]
    pub tip: bool,
    #[serde(default = "yes")]
    pub confidence: bool,
    /// Normalized 0..1 when the pad reports pressure.
    #[serde(default)]
    pub pressure: Option<f32>,
}

#[derive(Debug, Clone)]
pub struct TouchFrame {
    pub contacts: Vec<Contact>,
    pub button: bool,
    pub scan_time: u32,
    /// Arrival time in microseconds on the monotonic clock of [`now_us`].
    pub t_us: u64,
    pub device: Arc<DeviceInfo>,
}

#[derive(Debug, Clone)]
pub enum InputEvent {
    Frame(TouchFrame),
    /// A touch pad became available (its stable id is `DeviceInfo::id`).
    DeviceArrived(Arc<DeviceInfo>),
    /// The device with this id went away.
    DeviceRemoved(String),
    /// The input thread could not start or died.
    Failed(String),
}

/// Microseconds on a monotonic clock shared by the input and engine threads.
pub fn now_us() -> u64 {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    EPOCH.get_or_init(Instant::now).elapsed().as_micros() as u64
}

/// Counters the input thread updates for the diagnostics page.
#[derive(Debug, Default)]
pub struct InputCounters {
    /// Frames dropped because a later report never arrived or another ScanTime came first.
    pub partial_frames: AtomicU64,
    /// Contacts dropped because X, Y, Contact ID or Tip Switch could not be read.
    pub contacts: AtomicU64,
}

impl InputCounters {
    pub fn partial_frames(&self) -> u64 {
        self.partial_frames.load(Ordering::Relaxed)
    }

    pub fn contacts(&self) -> u64 {
        self.contacts.load(Ordering::Relaxed)
    }
}

/// Owner of the input thread: the event receiver plus stop and join.
pub struct InputHandle {
    rx: Option<Receiver<InputEvent>>,
    pub counters: Arc<InputCounters>,
    stopper: Box<dyn Fn() + Send + Sync>,
    join: Option<JoinHandle<()>>,
    /// Keeps the channel open for builds without an input source.
    _keep: Option<std::sync::mpsc::Sender<InputEvent>>,
}

impl InputHandle {
    /// Hand the event receiver to its (single) consumer.
    pub fn take_rx(&mut self) -> Option<Receiver<InputEvent>> {
        self.rx.take()
    }

    /// Ask the thread to end (closes its message-only window).
    pub fn stop(&self) {
        (self.stopper)();
    }

    /// Wait for the thread to end. Call after [`stop`](Self::stop).
    pub fn join(&mut self) {
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

/// Start the input thread. `wake` is called after each event so the consumer can react.
#[cfg(windows)]
pub fn spawn(wake: impl Fn() + Send + 'static) -> InputHandle {
    rawinput::spawn(wake)
}

/// Non-Windows builds have no input source; the receiver stays silent.
#[cfg(not(windows))]
pub fn spawn(_wake: impl Fn() + Send + 'static) -> InputHandle {
    let (tx, rx) = std::sync::mpsc::channel();
    InputHandle {
        rx: Some(rx),
        counters: Arc::default(),
        stopper: Box::new(|| {}),
        join: None,
        _keep: Some(tx),
    }
}

/// Map `v` from `min..=max` to 0..1 (clamped). A degenerate range gives 0.
pub fn normalize(v: i32, min: i32, max: i32) -> f32 {
    if max <= min {
        return 0.0;
    }
    ((v as f32 - min as f32) / (max as f32 - min as f32)).clamp(0.0, 1.0)
}

/// How a report's ScanTime is known.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scan {
    /// The device has no ScanTime usage: reports are joined by count only.
    Absent,
    /// The device has a ScanTime usage but reading it failed: the report cannot be joined.
    Unreadable,
    At(u32),
}

/// Joins contacts that a hybrid-reporting pad spreads over several reports.
///
/// Every report carries a fixed number of finger slots. The first report of a
/// frame announces the total contact count; follow-up reports announce 0 and
/// repeat the frame's ScanTime. A frame that cannot be completed is dropped.
#[derive(Debug, Default)]
pub struct FrameAccumulator {
    pending: Vec<Contact>,
    /// Slots consumed so far, including unreadable ones.
    taken: usize,
    expected: usize,
    scan_time: Option<u32>,
    /// ScanTime of the last completed frame; a later announced-0 report with the same value is its trailing report.
    last_done: Option<u32>,
    partial_dropped: u64,
    contacts_dropped: u64,
}

impl FrameAccumulator {
    /// Feed one report's finger slots (`None` = unreadable), its announced contact count and ScanTime.
    /// Returns the finished contact list once all announced contacts arrived.
    pub fn push(
        &mut self,
        slots: Vec<Option<Contact>>,
        announced: usize,
        scan: Scan,
    ) -> Option<Vec<Contact>> {
        let scan_time = match scan {
            Scan::Absent => None,
            Scan::At(t) => Some(t),
            Scan::Unreadable => {
                // Cannot be joined with anything. Only a self-contained report passes.
                if announced > 0 {
                    if self.expected > 0 {
                        self.partial_dropped += 1;
                    }
                    self.reset();
                    if slots.len() >= announced {
                        self.expected = announced;
                        let out = self.take_slots(slots);
                        self.last_done = None;
                        return out;
                    }
                    self.partial_dropped += 1;
                    return None;
                }
                // A follow-up (or a bare "all lifted") whose frame cannot be identified.
                self.partial_dropped += 1;
                self.reset();
                return None;
            }
        };
        if announced > 0 {
            if self.expected > 0 {
                // The previous frame never got its remaining reports.
                self.partial_dropped += 1;
            }
            self.reset();
            self.expected = announced;
            self.scan_time = scan_time;
        } else if self.expected == 0 {
            if scan_time.is_some() && scan_time == self.last_done {
                // Trailing report of a frame that is already complete.
                return None;
            }
            // Nothing in progress and nothing announced: all fingers lifted.
            self.last_done = scan_time;
            return Some(Vec::new());
        } else if scan_time != self.scan_time {
            // A follow-up of some other frame: the one in progress lost its tail.
            self.partial_dropped += 1;
            self.reset();
            return None;
        }
        let out = self.take_slots(slots);
        if out.is_some() {
            self.last_done = scan_time;
        }
        out
    }

    /// Count slots that were unreadable and removed before they reached [`push`](Self::push).
    pub fn note_unreadable(&mut self, n: usize) {
        self.contacts_dropped += n as u64;
    }

    fn take_slots(&mut self, slots: Vec<Option<Contact>>) -> Option<Vec<Contact>> {
        let room = self.expected - self.taken;
        for slot in slots.into_iter().take(room) {
            self.taken += 1;
            match slot {
                Some(c) => self.pending.push(c),
                None => self.contacts_dropped += 1,
            }
        }
        if self.taken >= self.expected {
            self.expected = 0;
            self.taken = 0;
            Some(std::mem::take(&mut self.pending))
        } else {
            None
        }
    }

    fn reset(&mut self) {
        self.pending.clear();
        self.taken = 0;
        self.expected = 0;
    }

    /// Counts since the last call: (partial frames dropped, contacts dropped).
    pub fn take_dropped(&mut self) -> (u64, u64) {
        (
            std::mem::take(&mut self.partial_dropped),
            std::mem::take(&mut self.contacts_dropped),
        )
    }
}

/// Frames-per-second over a rolling one second window.
#[derive(Debug, Default)]
pub struct FpsCounter {
    stamps: VecDeque<f64>,
}

impl FpsCounter {
    pub fn record(&mut self, now: f64) {
        self.stamps.push_back(now);
        self.trim(now);
    }

    pub fn rate(&mut self, now: f64) -> usize {
        self.trim(now);
        self.stamps.len()
    }

    fn trim(&mut self, now: f64) {
        while self.stamps.front().is_some_and(|&t| now - t > 1.0) {
            self.stamps.pop_front();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contact(id: u32) -> Contact {
        Contact {
            id,
            x: 0.5,
            y: 0.5,
            x_mm: 80.0,
            y_mm: 57.0,
            tip: true,
            confidence: true,
            pressure: None,
        }
    }

    fn slots(ids: &[u32]) -> Vec<Option<Contact>> {
        ids.iter().map(|&i| Some(contact(i))).collect()
    }

    fn ids(v: &[Contact]) -> Vec<u32> {
        v.iter().map(|c| c.id).collect()
    }

    #[test]
    fn normalize_clamps_and_scales() {
        assert_eq!(normalize(0, 0, 100), 0.0);
        assert_eq!(normalize(50, 0, 100), 0.5);
        assert_eq!(normalize(200, 0, 100), 1.0);
        assert_eq!(normalize(-5, -10, 10), 0.25);
        assert_eq!(normalize(5, 10, 10), 0.0);
    }

    #[test]
    fn single_report_frame() {
        let mut acc = FrameAccumulator::default();
        let out = acc.push(slots(&[1, 2, 9]), 2, Scan::At(100)).unwrap();
        assert_eq!(ids(&out), vec![1, 2]);
        assert_eq!(acc.take_dropped(), (0, 0));
    }

    #[test]
    fn five_contacts_over_two_reports() {
        let mut acc = FrameAccumulator::default();
        assert!(acc.push(slots(&[1, 2, 3]), 5, Scan::At(7)).is_none());
        let out = acc.push(slots(&[4, 5, 0]), 0, Scan::At(7)).unwrap();
        assert_eq!(ids(&out), vec![1, 2, 3, 4, 5]);
        assert_eq!(acc.take_dropped(), (0, 0));
    }

    #[test]
    fn missing_second_report_drops_partial_frame() {
        let mut acc = FrameAccumulator::default();
        assert!(acc.push(slots(&[1, 2, 3]), 5, Scan::At(7)).is_none());
        // The next frame starts before the tail of the previous one arrived.
        let out = acc.push(slots(&[8]), 1, Scan::At(8)).unwrap();
        assert_eq!(ids(&out), vec![8]);
        assert_eq!(acc.take_dropped(), (1, 0));
    }

    #[test]
    fn follow_up_with_other_scan_time_is_dropped() {
        let mut acc = FrameAccumulator::default();
        assert!(acc.push(slots(&[1, 2, 3]), 5, Scan::At(7)).is_none());
        assert!(acc.push(slots(&[4, 5, 0]), 0, Scan::At(6)).is_none());
        assert_eq!(acc.take_dropped(), (1, 0));
        // Nothing is in progress and the tail's frame never completed: reads as "all lifted".
        assert_eq!(
            acc.push(slots(&[4, 5, 0]), 0, Scan::At(7)),
            Some(Vec::new())
        );
        // A fresh frame works again.
        let out = acc.push(slots(&[1, 2]), 2, Scan::At(9)).unwrap();
        assert_eq!(ids(&out), vec![1, 2]);
    }

    #[test]
    fn trailing_report_of_completed_frame_is_ignored() {
        let mut acc = FrameAccumulator::default();
        // Frame completes in the first report; a trailing announced-0 report repeats its ScanTime.
        assert!(acc.push(slots(&[1, 2]), 2, Scan::At(7)).is_some());
        assert_eq!(acc.push(slots(&[0, 0]), 0, Scan::At(7)), None);
        assert_eq!(acc.take_dropped(), (0, 0));
        // A real "all lifted" has another ScanTime.
        assert_eq!(acc.push(slots(&[0]), 0, Scan::At(8)), Some(Vec::new()));
        // Repeats of that one are ignored too.
        assert_eq!(acc.push(slots(&[0]), 0, Scan::At(8)), None);
    }

    #[test]
    fn unreadable_scan_time_passes_only_complete_reports() {
        let mut acc = FrameAccumulator::default();
        let out = acc.push(slots(&[1, 2, 9]), 2, Scan::Unreadable).unwrap();
        assert_eq!(ids(&out), vec![1, 2]);
        assert_eq!(acc.take_dropped(), (0, 0));
        // Announces more than the report carries: cannot be joined.
        assert!(acc.push(slots(&[1, 2, 3]), 5, Scan::Unreadable).is_none());
        assert!(acc.push(slots(&[4, 5, 0]), 0, Scan::Unreadable).is_none());
        assert_eq!(acc.take_dropped().0, 2);
    }

    #[test]
    fn absent_scan_time_joins_by_count() {
        let mut acc = FrameAccumulator::default();
        assert!(acc.push(slots(&[1, 2, 3]), 5, Scan::Absent).is_none());
        let out = acc.push(slots(&[4, 5, 0]), 0, Scan::Absent).unwrap();
        assert_eq!(ids(&out), vec![1, 2, 3, 4, 5]);
        assert_eq!(acc.push(slots(&[0]), 0, Scan::Absent), Some(Vec::new()));
    }

    #[test]
    fn zero_count_is_empty_frame() {
        let mut acc = FrameAccumulator::default();
        assert_eq!(acc.push(slots(&[0]), 0, Scan::At(1)), Some(Vec::new()));
    }

    #[test]
    fn unreadable_slot_is_dropped_and_counted() {
        let mut acc = FrameAccumulator::default();
        let s = vec![Some(contact(1)), None, Some(contact(3))];
        let out = acc.push(s, 3, Scan::At(1)).unwrap();
        assert_eq!(ids(&out), vec![1, 3]);
        assert_eq!(acc.take_dropped(), (0, 1));
    }

    #[test]
    fn extent_from_units() {
        // 1600 units of 0.01 cm = 160 mm.
        assert_eq!(extent_mm(0, 1600, 0x11, 0xE), Some(160.0));
        // 40 units of 0.1 inch = 4 inch.
        assert!((extent_mm(0, 40, 0x13, 0xF).unwrap() - 101.6).abs() < 0.01);
        assert_eq!(extent_mm(0, 0, 0x11, 0), None);
        assert_eq!(extent_mm(0, 100, 0, 0), None);
        assert_eq!(extent_mm(0, 1, 0x11, 0), None);
    }

    #[test]
    fn fps_rolling_window() {
        let mut fps = FpsCounter::default();
        for i in 0..10 {
            fps.record(i as f64 * 0.05);
        }
        assert_eq!(fps.rate(0.5), 10);
        assert_eq!(fps.rate(1.2), 6);
        assert_eq!(fps.rate(5.0), 0);
    }

    #[test]
    fn aspect_falls_back() {
        assert_eq!(DeviceInfo::default().aspect(), 1.6);
        let d = DeviceInfo {
            x_range: (0, 160),
            y_range: (0, 100),
            ..Default::default()
        };
        assert_eq!(d.aspect(), 1.6);
    }
}
