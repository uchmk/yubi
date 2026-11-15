//! Pressure calibration helper, pure logic (no UI, no clock): validating samples, suggesting
//! thresholds from a measured baseline and firm level, and the calibration wizard's state machine.
//! Nothing here gates the press gesture; thresholds are raw pressure.
//!
//! The pad reports pressure on a fixed 0..1 scale but real touches only span ~0.01..0.20, and the
//! force is split between contacts, so a firm press peaks lower with two or three fingers. A device
//! therefore stores one relaxed baseline `b` and a firm level per finger count (`f1`, `f2`, `f3`).

use crate::config::{Calibration, MIN_CALIBRATION_SPAN};

/// Samples per pass (relaxed, and firm for each finger count).
pub const SAMPLES_PER_PASS: usize = 5;
/// Length of one sample.
pub const SAMPLE_US: u64 = 1_000_000;
/// A silence this long inside a sample (the page was left, the pad stopped reporting) throws the
/// sample away; frames arrive every ~11 ms.
pub const MAX_FRAME_GAP_US: u64 = 200_000;
/// Highest finger count the wizard can calibrate a firm level for.
pub const MAX_CALIBRATED_FINGERS: usize = 5;

/// Suggested thresholds for a measured relaxed level `baseline` and firm level `firm`:
/// light at 40 % and deep at 80 % of the way, in whole hundredths inside the allowed range with
/// the minimum gap. A span that is not positive gives the defaults for one finger.
pub fn suggest(baseline: f32, firm: f32) -> (f32, f32) {
    if firm.partial_cmp(&baseline) != Some(std::cmp::Ordering::Greater)
        || !baseline.is_finite()
        || !firm.is_finite()
    {
        return crate::config::DEFAULT_ONE;
    }
    let at = |k: f64| (f64::from(baseline) + k * f64::from(firm - baseline)) as f32;
    crate::config::fit_pair(at(0.4), at(0.8))
}

/// `vvvv:pppp` (lowercase hex) from a device interface path such as
/// `\\?\HID#VID_05AC&PID_0324#...` or the Bluetooth form `...VID&0205ac_PID&0324...`.
pub fn device_key(id: &str) -> Option<String> {
    let upper = id.to_ascii_uppercase();
    let hex_after = |tag: &str| -> Option<String> {
        let at = upper.find(tag)? + tag.len();
        let run: String = upper[at..]
            .chars()
            .skip_while(|c| !c.is_ascii_hexdigit())
            .take_while(char::is_ascii_hexdigit)
            .collect();
        (run.len() >= 4).then(|| run[run.len() - 4..].to_ascii_lowercase())
    };
    Some(format!("{}:{}", hex_after("VID")?, hex_after("PID")?))
}

/// Why a calibration was refused.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Reject {
    /// The pad reported no pressure.
    Missing,
    /// The value never changed.
    Stuck,
    /// Firm minus relaxed is below [`MIN_CALIBRATION_SPAN`].
    TooClose,
    /// The firm readings reach down into the relaxed ones.
    Overlap,
}

impl Reject {
    pub fn message(self) -> &'static str {
        match self {
            Self::Missing => "このトラックパッドから圧力の値が届いていません",
            Self::Stuck => {
                "圧力の値がまったく変わりません。トラックパッドを押し込めているか確認してください"
            }
            Self::TooClose => {
                "軽く触れたときとしっかり押したときの差が小さすぎます（0.05 未満）。もう少し強く押してください"
            }
            Self::Overlap => {
                "軽く触れたときとしっかり押したときの値が重なっています。軽い触れ方を一定にして、やり直してください"
            }
        }
    }
}

pub fn median(values: &[f32]) -> Option<f32> {
    let mut v: Vec<f32> = values.iter().copied().filter(|x| x.is_finite()).collect();
    if v.is_empty() {
        return None;
    }
    v.sort_by(f32::total_cmp);
    let n = v.len();
    Some(if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    })
}

/// Nearest-rank percentile (`pct` in 0..=100).
pub fn percentile(values: &[f32], pct: f32) -> Option<f32> {
    let mut v: Vec<f32> = values.iter().copied().filter(|x| x.is_finite()).collect();
    if v.is_empty() {
        return None;
    }
    v.sort_by(f32::total_cmp);
    let rank = ((pct.clamp(0.0, 100.0) / 100.0) * (v.len() - 1) as f32).round() as usize;
    Some(v[rank])
}

/// The stable middle of a sample: readings from the middle half of its duration, so landing and
/// lifting ramps do not count. `samples` are `(t_us, value)` in time order.
pub fn stable_middle(samples: &[(u64, f32)]) -> Vec<f32> {
    let (Some(first), Some(last)) = (samples.first(), samples.last()) else {
        return Vec::new();
    };
    let span = last.0.saturating_sub(first.0);
    let (lo, hi) = (first.0 + span / 4, first.0 + span - span / 4);
    samples
        .iter()
        .filter(|(t, _)| (lo..=hi).contains(t))
        .map(|(_, v)| *v)
        .collect()
}

/// Level of one pass: the median of the per-sample medians (robust against one odd sample and
/// against a peak that wobbles with the other fingers). Pass the stable middle of each sample.
pub fn pass_level(samples: &[Vec<f32>]) -> Option<f32> {
    let per: Vec<f32> = samples.iter().filter_map(|s| median(s)).collect();
    median(&per)
}

/// Fit one firm level against the relaxed baseline.
/// Returns `(baseline, firm)` on the raw scale, rounded to 0.01.
pub fn fit(relaxed: &[Vec<f32>], firm: &[Vec<f32>]) -> Result<(f32, f32), Reject> {
    let rel: Vec<f32> = relaxed.iter().flatten().copied().collect();
    let fm: Vec<f32> = firm.iter().flatten().copied().collect();
    if rel.is_empty() || fm.is_empty() {
        return Err(Reject::Missing);
    }
    let (lo, hi) = rel
        .iter()
        .chain(&fm)
        .fold((f32::MAX, f32::MIN), |(l, h), v| (l.min(*v), h.max(*v)));
    if hi - lo < 1e-4 {
        return Err(Reject::Stuck);
    }
    let b = pass_level(relaxed).ok_or(Reject::Missing)?;
    let f = pass_level(firm).ok_or(Reject::Missing)?;
    if f - b < MIN_CALIBRATION_SPAN {
        return Err(Reject::TooClose);
    }
    if percentile(&fm, 20.0) <= percentile(&rel, 80.0) {
        return Err(Reject::Overlap);
    }
    let q = crate::config::q01;
    let (b, f) = (q(b), q(f));
    if f - b < MIN_CALIBRATION_SPAN - 1e-4 {
        return Err(Reject::TooClose);
    }
    Ok((b, f))
}

/// What the wizard collects now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// One finger resting lightly.
    Relaxed,
    /// This many fingers pressing firmly.
    Firm(usize),
}

impl Step {
    /// Contacts required while sampling.
    pub fn fingers(self) -> usize {
        match self {
            Self::Relaxed => 1,
            Self::Firm(n) => n,
        }
    }

    /// The instruction for the sample being taken (Japanese).
    pub fn instruction(self) -> String {
        match self {
            Self::Relaxed => "1 本指を軽く触れたままにしてください".into(),
            Self::Firm(1) => "1 本指でしっかり押してください".into(),
            Self::Firm(n) => format!("{n} 本指でしっかり押してください"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Phase {
    Idle,
    /// Waiting for the right number of fingers.
    Armed,
    /// Sampling; `elapsed_us` of [`SAMPLE_US`].
    Sampling {
        elapsed_us: u64,
    },
    /// A sample finished: lift all fingers before the next.
    WaitLift,
    /// A firm pass for `n` fingers is done; offer the next count (or finish).
    Offer(usize),
    Failed(Reject),
    Done,
}

/// The inline calibration wizard: 5 relaxed samples, 5 firm samples with one finger, then
/// optionally 5 firm samples each with two to five fingers.
#[derive(Debug, Clone, PartialEq)]
pub struct Wizard {
    step: Step,
    phase: Phase,
    /// Stable middle of every finished sample of the current pass.
    pass: Vec<Vec<f32>>,
    relaxed: Vec<Vec<f32>>,
    /// Firm level per finger count found so far (index = count - 1).
    firm: [Option<f32>; MAX_CALIBRATED_FINGERS],
    baseline: Option<f32>,
    current: Vec<(u64, f32)>,
    sampling_since: Option<u64>,
    /// A one-line hint about the last discarded sample.
    note: Option<&'static str>,
}

impl Default for Wizard {
    fn default() -> Self {
        Self {
            step: Step::Relaxed,
            phase: Phase::Idle,
            pass: Vec::new(),
            relaxed: Vec::new(),
            firm: [None; MAX_CALIBRATED_FINGERS],
            baseline: None,
            current: Vec::new(),
            sampling_since: None,
            note: None,
        }
    }
}

impl Wizard {
    /// Start over from the relaxed pass.
    pub fn start(&mut self) {
        *self = Self::default();
        self.phase = Phase::Armed;
    }

    /// Start only the firm pass for `n` fingers (calibrate or redo one count); needs a baseline
    /// from `existing`.
    pub fn start_count(&mut self, n: usize, existing: &Calibration) {
        *self = Self::default();
        self.baseline = Some(existing.baseline);
        self.firm = [
            Some(existing.f1),
            existing.f2,
            existing.f3,
            existing.f4,
            existing.f5,
        ];
        self.relaxed = vec![vec![existing.baseline]];
        self.step = Step::Firm(n.clamp(1, MAX_CALIBRATED_FINGERS));
        self.phase = Phase::Armed;
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    pub fn step(&self) -> Step {
        self.step
    }

    pub fn note(&self) -> Option<&'static str> {
        self.note
    }

    pub fn active(&self) -> bool {
        !matches!(self.phase, Phase::Idle | Phase::Done)
    }

    /// Finished samples in the current pass.
    pub fn samples_done(&self) -> usize {
        self.pass.len()
    }

    /// 0..1 progress through the whole current pass.
    pub fn progress(&self) -> f32 {
        let part = match self.phase {
            Phase::Sampling { elapsed_us } => elapsed_us as f32 / SAMPLE_US as f32,
            _ => 0.0,
        };
        ((self.pass.len() as f32 + part) / SAMPLES_PER_PASS as f32).clamp(0.0, 1.0)
    }

    #[cfg(test)]
    pub fn is_done(&self) -> bool {
        self.phase == Phase::Done
    }

    /// The calibration found so far (needs at least the one-finger firm level).
    pub fn result(&self, date: &str) -> Option<Calibration> {
        Some(Calibration {
            baseline: self.baseline?,
            f1: self.firm[0]?,
            f2: self.firm[1],
            f3: self.firm[2],
            f4: self.firm[3],
            f5: self.firm[4],
            date: date.to_owned(),
        })
    }

    /// Take the next count offered after a finished firm pass.
    pub fn accept_offer(&mut self) {
        if let Phase::Offer(next) = self.phase {
            self.step = Step::Firm(next);
            self.begin_pass();
        }
    }

    /// Stop offering more counts: the calibration is complete.
    pub fn skip_offer(&mut self) {
        if matches!(self.phase, Phase::Offer(_)) {
            self.phase = Phase::Done;
        }
    }

    /// Redo the pass that failed (the relaxed pass restarts the whole wizard).
    pub fn retry(&mut self) {
        if let Phase::Failed(_) = self.phase {
            match self.step {
                Step::Firm(n) if n > 1 || self.baseline.is_some() && self.firm[0].is_some() => {
                    self.begin_pass()
                }
                _ => self.start(),
            }
        }
    }

    fn begin_pass(&mut self) {
        self.pass.clear();
        self.current.clear();
        self.sampling_since = None;
        self.note = None;
        self.phase = Phase::Armed;
    }

    /// The wizard was not fed for a while (its page was left): drop the sample in progress so
    /// coming back starts a fresh one instead of reading a stale clock.
    pub fn interrupt(&mut self) {
        if matches!(self.phase, Phase::Sampling { .. }) {
            self.drop_sample("しばらく操作がなかったため、この回はやり直します");
        }
    }

    fn drop_sample(&mut self, note: &'static str) {
        self.current.clear();
        self.sampling_since = None;
        self.note = Some(note);
        self.phase = Phase::Armed;
    }

    /// Feed one frame: `t_us` and the pressure of every active contact (`None` = the pad sent no
    /// pressure). Call with an empty slice when no finger is down.
    pub fn feed(&mut self, t_us: u64, contacts: &[Option<f32>]) {
        let n = contacts.len();
        match self.phase {
            Phase::Idle | Phase::Done | Phase::Offer(_) | Phase::Failed(_) => {}
            Phase::WaitLift => {
                if n == 0 {
                    self.phase = Phase::Armed;
                }
            }
            Phase::Armed => {
                if n == self.step.fingers() {
                    if contacts.iter().any(Option::is_none) {
                        self.phase = Phase::Failed(Reject::Missing);
                        return;
                    }
                    self.current.clear();
                    self.sampling_since = Some(t_us);
                    self.phase = Phase::Sampling { elapsed_us: 0 };
                    self.note = None;
                    self.push(t_us, contacts);
                }
            }
            Phase::Sampling { .. } => {
                if n != self.step.fingers() {
                    // A finger came or went: throw the sample away and wait again.
                    self.drop_sample("途中で指が変わったため、この回はやり直します");
                    return;
                }
                let last = self.current.last().map_or(t_us, |&(t, _)| t);
                if t_us.saturating_sub(last) > MAX_FRAME_GAP_US {
                    // The frames stopped for a while: the clock of this sample is stale.
                    self.drop_sample("しばらく操作がなかったため、この回はやり直します");
                    return;
                }
                self.push(t_us, contacts);
                let since = self.sampling_since.unwrap_or(t_us);
                let elapsed = t_us.saturating_sub(since);
                if elapsed >= SAMPLE_US {
                    self.finish_sample();
                } else {
                    self.phase = Phase::Sampling {
                        elapsed_us: elapsed,
                    };
                }
            }
        }
    }

    fn push(&mut self, t_us: u64, contacts: &[Option<f32>]) {
        let top = contacts
            .iter()
            .flatten()
            .copied()
            .filter(|v| v.is_finite())
            .fold(f32::MIN, f32::max);
        if top > f32::MIN {
            self.current.push((t_us, top));
        }
    }

    fn finish_sample(&mut self) {
        let mid = stable_middle(&self.current);
        self.current.clear();
        self.sampling_since = None;
        if mid.is_empty() {
            self.phase = Phase::Failed(Reject::Missing);
            return;
        }
        self.pass.push(mid);
        if self.pass.len() < SAMPLES_PER_PASS {
            self.phase = Phase::WaitLift;
            return;
        }
        self.finish_pass();
    }

    fn finish_pass(&mut self) {
        let pass = std::mem::take(&mut self.pass);
        match self.step {
            Step::Relaxed => {
                self.relaxed = pass;
                self.step = Step::Firm(1);
                self.begin_pass();
                self.phase = Phase::WaitLift;
            }
            Step::Firm(n) => match fit(&self.relaxed, &pass) {
                Ok((b, f)) => {
                    // A one-finger pass fixes the baseline; later counts reuse it.
                    if n == 1 {
                        self.baseline = Some(b);
                    }
                    let base = self.baseline.unwrap_or(b);
                    if f - base < MIN_CALIBRATION_SPAN - 1e-4 {
                        self.pass = pass;
                        self.phase = Phase::Failed(Reject::TooClose);
                        return;
                    }
                    self.firm[n - 1] = Some(f);
                    self.phase = if n < MAX_CALIBRATED_FINGERS {
                        Phase::Offer(n + 1)
                    } else {
                        Phase::Done
                    };
                }
                Err(r) => {
                    self.phase = Phase::Failed(r);
                }
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggestion_is_forty_and_eighty_percent_rounded_to_hundredths() {
        assert_eq!(suggest(0.01, 0.16), (0.07, 0.13));
        assert_eq!(suggest(0.01, 0.20), (0.09, 0.16));
        assert_eq!(suggest(0.02, 0.12), (0.06, 0.10));
    }

    #[test]
    fn suggestion_keeps_range_and_gap() {
        // A small span still leaves the minimum gap.
        let (l, d) = suggest(0.05, 0.08);
        assert!(d - l >= crate::config::MIN_THRESHOLD_GAP - 1e-6, "{l} {d}");
        let (l, d) = suggest(0.18, 0.20);
        assert_eq!((l, d), (0.18, 0.20));
        let (l, d) = suggest(0.0, 0.2);
        assert!(l >= 0.01 && d <= 0.20);
        // Nonsense input gives the defaults.
        assert_eq!(suggest(0.2, 0.1), crate::config::DEFAULT_ONE);
        assert_eq!(suggest(f32::NAN, 0.1), crate::config::DEFAULT_ONE);
    }

    #[test]
    fn device_key_reads_vid_and_pid() {
        assert_eq!(
            device_key(r"\\?\HID#VID_05AC&PID_0324#8&2a#{4d1e}").as_deref(),
            Some("05ac:0324")
        );
        assert_eq!(
            device_key(r"\\?\HID#{00001124-0000-1000-8000-00805f9b34fb}_VID&0205ac_PID&0324#a&b")
                .as_deref(),
            Some("05ac:0324")
        );
        assert_eq!(device_key("nothing"), None);
    }

    fn pass(level: f32, jitter: f32) -> Vec<Vec<f32>> {
        (0..5)
            .map(|i| {
                (0..20)
                    .map(|k| level + jitter * if (i + k) % 2 == 0 { 1.0 } else { -1.0 })
                    .collect()
            })
            .collect()
    }

    #[test]
    fn fit_accepts_the_measured_range() {
        let (b, f) = fit(&pass(0.01, 0.003), &pass(0.20, 0.01)).unwrap();
        assert_eq!((b, f), (0.01, 0.2));
        // Two and three fingers peak lower but stay usable.
        let (_, f2) = fit(&pass(0.01, 0.003), &pass(0.16, 0.01)).unwrap();
        assert_eq!(f2, 0.16);
        let (_, f3) = fit(&pass(0.01, 0.003), &pass(0.15, 0.01)).unwrap();
        assert_eq!(f3, 0.15);
    }

    #[test]
    fn fit_rejects_a_small_span_missing_stuck_and_overlap() {
        assert_eq!(
            fit(&pass(0.01, 0.002), &pass(0.05, 0.002)),
            Err(Reject::TooClose)
        );
        assert_eq!(fit(&[], &pass(0.2, 0.01)), Err(Reject::Missing));
        assert_eq!(fit(&pass(0.1, 0.0), &pass(0.1, 0.0)), Err(Reject::Stuck));
        // Wide jitter makes the firm 20th percentile reach the relaxed 80th percentile.
        assert_eq!(
            fit(&pass(0.01, 0.08), &pass(0.12, 0.08)),
            Err(Reject::Overlap)
        );
    }

    #[test]
    fn median_and_percentile() {
        assert_eq!(median(&[3.0, 1.0, 2.0]), Some(2.0));
        assert_eq!(median(&[1.0, 2.0, 3.0, 4.0]), Some(2.5));
        assert_eq!(median(&[]), None);
        assert_eq!(percentile(&[1.0, 2.0, 3.0, 4.0, 5.0], 20.0), Some(2.0));
        assert_eq!(percentile(&[1.0, 2.0, 3.0, 4.0, 5.0], 80.0), Some(4.0));
    }

    #[test]
    fn stable_middle_drops_landing_and_lift_ramps() {
        let s: Vec<(u64, f32)> = (0..=10)
            .map(|i| (i * 100_000, if !(2..=8).contains(&i) { 0.0 } else { 0.2 }))
            .collect();
        let mid = stable_middle(&s);
        assert!(mid.iter().all(|v| *v == 0.2), "{mid:?}");
        assert!(!mid.is_empty());
    }

    #[test]
    fn firm_level_is_the_median_not_the_peak() {
        // One sample with a brief spike must not set the level.
        let mut p = pass(0.15, 0.005);
        p[2].push(0.40);
        let level = pass_level(&p).unwrap();
        assert!((level - 0.15).abs() < 0.01, "{level}");
    }

    /// Drive the wizard with `n` fingers at `level` for one sample, then a lift.
    fn sample(w: &mut Wizard, t: &mut u64, n: usize, level: f32) {
        let down = vec![Some(level); n];
        let mut elapsed = 0;
        while elapsed <= SAMPLE_US + 20_000 {
            w.feed(*t, &down);
            *t += 10_000;
            elapsed += 10_000;
        }
        w.feed(*t, &[]);
        *t += 10_000;
    }

    #[test]
    fn wizard_runs_one_two_and_three_fingers() {
        let mut w = Wizard::default();
        w.start();
        let mut t = 0;
        for _ in 0..5 {
            sample(&mut w, &mut t, 1, 0.01);
        }
        assert_eq!(w.step(), Step::Firm(1));
        for _ in 0..5 {
            sample(&mut w, &mut t, 1, 0.20);
        }
        assert_eq!(w.phase(), Phase::Offer(2));
        let c = w.result("2026-11-15").unwrap();
        assert_eq!((c.baseline, c.f1, c.f2, c.f3), (0.01, 0.2, None, None));
        w.accept_offer();
        assert_eq!(w.step(), Step::Firm(2));
        for _ in 0..5 {
            sample(&mut w, &mut t, 2, 0.16);
        }
        assert_eq!(w.phase(), Phase::Offer(3));
        w.skip_offer();
        assert!(w.is_done());
        let c = w.result("2026-11-15").unwrap();
        assert_eq!((c.f2, c.f3), (Some(0.16), None));
        assert!(c.usable(2) && !c.usable(3));
    }

    #[test]
    fn a_stale_sample_is_dropped_instead_of_failing() {
        let mut w = Wizard::default();
        w.start();
        w.feed(0, &[Some(0.02)]);
        w.feed(11_000, &[Some(0.02)]);
        assert!(matches!(w.phase(), Phase::Sampling { .. }));
        // The page was left for a minute; the next frame must not finish the sample.
        w.feed(60_000_000, &[Some(0.02)]);
        assert_eq!(w.phase(), Phase::Armed);
        assert!(w.note().is_some());
        // Coming back works: a fresh sample starts and runs normally.
        w.feed(60_011_000, &[Some(0.02)]);
        assert!(matches!(w.phase(), Phase::Sampling { .. }));
        // `interrupt` does the same when the page reports being left.
        w.interrupt();
        assert_eq!(w.phase(), Phase::Armed);
        assert_eq!(w.samples_done(), 0);
    }

    #[test]
    fn wizard_waits_for_the_right_finger_count_and_retries_early_lifts() {
        let mut w = Wizard::default();
        w.start();
        // Two fingers during the one-finger relaxed pass: nothing is sampled.
        w.feed(0, &[Some(0.01), Some(0.01)]);
        assert_eq!(w.phase(), Phase::Armed);
        w.feed(10_000, &[Some(0.01)]);
        assert!(matches!(w.phase(), Phase::Sampling { .. }));
        // Lifting after 300 ms discards the sample.
        w.feed(300_000, &[Some(0.01)]);
        w.feed(310_000, &[]);
        assert_eq!(w.phase(), Phase::Armed);
        assert_eq!(w.samples_done(), 0);
        assert!(w.note().is_some());
    }

    #[test]
    fn wizard_rejects_a_weak_firm_pass_and_missing_pressure() {
        let mut w = Wizard::default();
        w.start();
        let mut t = 0;
        for _ in 0..5 {
            sample(&mut w, &mut t, 1, 0.01);
        }
        for _ in 0..5 {
            sample(&mut w, &mut t, 1, 0.04);
        }
        assert_eq!(w.phase(), Phase::Failed(Reject::TooClose));
        w.retry();
        assert_eq!(w.step(), Step::Relaxed);
        assert_eq!(w.phase(), Phase::Armed);
        let mut w = Wizard::default();
        w.start();
        w.feed(0, &[None]);
        assert_eq!(w.phase(), Phase::Failed(Reject::Missing));
    }

    #[test]
    fn calibrating_one_more_count_keeps_the_rest() {
        let existing = Calibration {
            baseline: 0.01,
            f1: 0.2,
            f2: None,
            f3: None,
            f4: None,
            f5: None,
            date: "d".into(),
        };
        let mut w = Wizard::default();
        w.start_count(2, &existing);
        let mut t = 0;
        for _ in 0..5 {
            sample(&mut w, &mut t, 2, 0.16);
        }
        let c = w.result("e").unwrap();
        assert_eq!((c.baseline, c.f1, c.f2), (0.01, 0.2, Some(0.16)));
    }
}
