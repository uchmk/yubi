//! Trackpad input: platform-independent types and pure helpers.
//! The Windows Raw Input reader lives in `rawinput` and feeds `InputEvent`s
//! over a channel; nothing here depends on the UI.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::mpsc::Receiver;

#[cfg(windows)]
mod rawinput;

/// Static description of a touch pad as reported by its HID descriptor.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DeviceInfo {
    pub name: String,
    /// Logical (min, max) of X and Y.
    pub x_range: (i32, i32),
    pub y_range: (i32, i32),
    /// Logical (min, max) of tip pressure, if the pad reports it.
    pub pressure_range: Option<(i32, i32)>,
    /// Number of finger slots per report.
    pub finger_slots: usize,
}

impl DeviceInfo {
    /// Width / height of the sensing area, falling back to 16:10.
    pub fn aspect(&self) -> f32 {
        let w = (i64::from(self.x_range.1) - i64::from(self.x_range.0)) as f32;
        let h = (i64::from(self.y_range.1) - i64::from(self.y_range.0)) as f32;
        if w > 0.0 && h > 0.0 { w / h } else { 1.6 }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Contact {
    pub id: u32,
    /// Normalized 0..1 (left to right).
    pub x: f32,
    /// Normalized 0..1 (top to bottom).
    pub y: f32,
    pub tip: bool,
    pub confidence: bool,
    /// Normalized 0..1 when the pad reports pressure.
    pub pressure: Option<f32>,
}

#[derive(Debug, Clone)]
pub struct TouchFrame {
    pub contacts: Vec<Contact>,
    pub button: bool,
    pub scan_time: u32,
    pub device: Arc<DeviceInfo>,
}

#[derive(Debug, Clone)]
pub enum InputEvent {
    Frame(TouchFrame),
    DeviceArrived(Arc<DeviceInfo>),
    DeviceRemoved(String),
}

/// Start the input thread. `wake` is called after each event so the UI can repaint.
#[cfg(windows)]
pub fn spawn(wake: impl Fn() + Send + 'static) -> Receiver<InputEvent> {
    rawinput::spawn(wake)
}

/// Non-Windows builds have no input source; the receiver stays silent.
#[cfg(not(windows))]
pub fn spawn(_wake: impl Fn() + Send + 'static) -> Receiver<InputEvent> {
    std::sync::mpsc::channel().1
}

/// Map `v` from `min..=max` to 0..1 (clamped). A degenerate range gives 0.
pub fn normalize(v: i32, min: i32, max: i32) -> f32 {
    if max <= min {
        return 0.0;
    }
    ((v as f32 - min as f32) / (max as f32 - min as f32)).clamp(0.0, 1.0)
}

/// Joins contacts that a hybrid-reporting pad spreads over several reports.
///
/// Every report carries a fixed number of finger slots. The first report of a
/// frame announces the total contact count; follow-up reports announce 0.
#[derive(Debug, Default)]
pub struct FrameAccumulator {
    pending: Vec<Contact>,
    expected: usize,
}

impl FrameAccumulator {
    /// Feed one report's finger slots and its announced contact count.
    /// Returns the finished contact list once all announced contacts arrived.
    pub fn push(&mut self, slots: Vec<Contact>, announced: usize) -> Option<Vec<Contact>> {
        if announced > 0 {
            self.pending.clear();
            self.expected = announced;
        } else if self.pending.len() >= self.expected {
            // Nothing in progress and nothing announced: all fingers lifted.
            self.pending.clear();
            self.expected = 0;
            return Some(Vec::new());
        }
        let room = self.expected - self.pending.len();
        self.pending.extend(slots.into_iter().take(room));
        if self.pending.len() >= self.expected {
            self.expected = 0;
            Some(std::mem::take(&mut self.pending))
        } else {
            None
        }
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
            tip: true,
            confidence: true,
            pressure: None,
        }
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
        let out = acc
            .push(vec![contact(1), contact(2), contact(9)], 2)
            .unwrap();
        assert_eq!(ids(&out), vec![1, 2]);
    }

    #[test]
    fn hybrid_frame_spans_reports() {
        let mut acc = FrameAccumulator::default();
        assert!(acc.push(vec![contact(1), contact(2)], 3).is_none());
        let out = acc.push(vec![contact(3), contact(0)], 0).unwrap();
        assert_eq!(ids(&out), vec![1, 2, 3]);
    }

    #[test]
    fn zero_count_is_empty_frame() {
        let mut acc = FrameAccumulator::default();
        assert_eq!(acc.push(vec![contact(0)], 0), Some(Vec::new()));
    }

    #[test]
    fn new_announcement_restarts() {
        let mut acc = FrameAccumulator::default();
        assert!(acc.push(vec![contact(1)], 3).is_none());
        let out = acc.push(vec![contact(7)], 1).unwrap();
        assert_eq!(ids(&out), vec![7]);
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
