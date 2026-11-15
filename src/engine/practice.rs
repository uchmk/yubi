//! Practice view: what the recognizer recognized and what became of it.
//!
//! The engine feeds every recognizer action through [`Recorder::observe`] right before the
//! dispatcher sees it; the result is a short history the practice page shows. Nothing here
//! decides anything: it only describes. Pure (no clock of its own, no OS), so it is unit-tested.

use std::time::{Duration, Instant};

use super::dispatch::{corner_swipe_label, press_label};
use super::recognizer::{Action, GestureEvent};
use super::types::{EdgeSide, GestureId, Suppress};
use crate::config;
use crate::config::EdgeKind;

/// Entries the history keeps (newest first).
pub const HISTORY_MAX: usize = 10;

/// What happened to a recognized gesture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The dispatcher ran it.
    Ran,
    /// Practice mode was on: shown, not run.
    DryRun,
    /// The foreground app's profile switches yubi off.
    Blocked,
    /// Recognized, but no operation is attached (a plain tap, a long hold).
    NoBinding,
}

impl Outcome {
    pub fn note(self) -> &'static str {
        match self {
            Self::Ran => "実行しました",
            Self::DryRun => "練習モードのため実行しません",
            Self::Blocked => "このアプリでは yubi が無効のため実行しません",
            Self::NoBinding => "割り当てられた操作はありません",
        }
    }
}

/// One recognized gesture.
#[derive(Debug, Clone, PartialEq)]
pub struct Judgement {
    /// Japanese name, e.g. `3本指で上にスワイプ`.
    pub gesture: String,
    /// The id the config files use, e.g. `swipe.3.up`.
    pub token: String,
    /// The operation it would run (or ran), when there is one.
    pub action: Option<String>,
    pub outcome: Outcome,
    pub at: Instant,
    /// Continues the entry before it (same `token`): an edge slider reports one entry that
    /// follows the slide, not one per step.
    pub update: bool,
}

/// The newest judgements, newest first.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct History {
    items: Vec<Judgement>,
}

impl History {
    pub fn push(&mut self, j: Judgement) {
        // A slider's running total replaces its earlier entry. A tap that a bound multi-finger
        // tap also reports (same id) is one gesture: keep the richer entry.
        if self
            .items
            .first()
            .is_some_and(|last| last.token == j.token && (j.update || last.action.is_none()))
        {
            self.items.remove(0);
        }
        self.items.insert(0, j);
        self.items.truncate(HISTORY_MAX);
    }

    /// Only the entries recorded at or after `since`.
    pub fn since(&self, since: Instant) -> History {
        History {
            items: self
                .items
                .iter()
                .filter(|j| j.at >= since)
                .cloned()
                .collect(),
        }
    }

    pub fn latest(&self) -> Option<&Judgement> {
        self.items.first()
    }

    pub fn items(&self) -> &[Judgement] {
        &self.items
    }
}

/// How the dispatcher will treat the next action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Mode {
    pub dry_run: bool,
    pub blocked: bool,
}

impl Mode {
    fn outcome(self) -> Outcome {
        if self.blocked {
            Outcome::Blocked
        } else if self.dry_run {
            Outcome::DryRun
        } else {
            Outcome::Ran
        }
    }
}

/// Turns recognizer actions into judgements. A multi-finger gesture arrives as a `Multi` event
/// followed by its `Trigger`; the name waits here until the trigger shows up.
#[derive(Debug, Default)]
pub struct Recorder {
    pending: Option<(String, String)>,
    /// The edge slide in progress.
    edge: Option<EdgeRun>,
    /// A slider took the touch: the hold of the same touch is not a gesture of its own.
    edge_touch: bool,
}

/// The steps of one edge slide so far.
#[derive(Debug, Clone, Copy)]
struct EdgeRun {
    side: EdgeSide,
    kind: EdgeKind,
    /// Forward steps minus back steps.
    net: i32,
}

/// Short name of what an edge slider is set to.
fn edge_kind_name(kind: EdgeKind) -> &'static str {
    match kind {
        EdgeKind::None => "なし",
        EdgeKind::Volume => "音量",
        EdgeKind::Brightness => "明るさ",
        EdgeKind::Scroll => "スクロール",
        EdgeKind::Zoom => "ズーム",
        EdgeKind::SeekYoutube | EdgeKind::SeekGeneral => "早送り・巻き戻し",
        EdgeKind::UndoRedo => "元に戻す／やり直し",
        EdgeKind::Tab => "タブの切り替え",
        EdgeKind::Custom => "カスタム",
    }
}

impl EdgeRun {
    /// 「右端スライダー（音量）」.
    fn name(&self) -> String {
        format!(
            "{}スライダー（{}）",
            self.side.label(),
            edge_kind_name(self.kind)
        )
    }

    /// The direction and the net step count, e.g. 「上へ +3」, 「下へ −2」.
    fn travel(&self) -> String {
        let (fwd, back) = if self.side.horizontal() {
            ("右", "左")
        } else {
            ("上", "下")
        };
        match self.net {
            0 => "±0".to_owned(),
            n if n > 0 => format!("{fwd}へ +{n}"),
            n => format!("{back}へ −{}", -n),
        }
    }
}

impl Recorder {
    pub fn observe(&mut self, a: &Action, mode: Mode, at: Instant) -> Option<Judgement> {
        let made = |gesture: String, token: String, action: Option<String>, outcome| Judgement {
            gesture,
            token,
            action,
            outcome,
            at,
            update: false,
        };
        match a {
            Action::EdgeStep {
                side,
                kind,
                forward,
                ..
            } => {
                self.edge_touch = true;
                // Only a step of the slide already running continues its entry.
                let continuing = self.edge.is_some();
                let run = self.edge.get_or_insert(EdgeRun {
                    side: *side,
                    kind: *kind,
                    net: 0,
                });
                run.net += if *forward { 1 } else { -1 };
                let (gesture, travel) = (run.name(), run.travel());
                Some(Judgement {
                    update: continuing,
                    ..made(gesture, side.key().to_owned(), Some(travel), mode.outcome())
                })
            }
            Action::Suppress(Suppress::Off) => {
                self.edge = None;
                None
            }
            Action::Debug(GestureEvent::FingerCount(0) | GestureEvent::TouchEnd { .. }) => {
                self.edge = None;
                self.edge_touch = false;
                None
            }
            Action::Debug(GestureEvent::Multi { fingers, kind }) => {
                self.pending = Some((
                    kind.label(*fingers),
                    kind.gesture_id(*fingers).as_str().to_owned(),
                ));
                None
            }
            Action::Debug(GestureEvent::Corner { corner, slot }) => {
                self.pending = Some((
                    corner_swipe_label(*corner, *slot),
                    GestureId::corner_slot(*corner, *slot).as_str().to_owned(),
                ));
                None
            }
            Action::CornerResizeBegin { .. } => {
                let (gesture, token) = self
                    .pending
                    .take()
                    .unwrap_or_else(|| ("ジェスチャー".to_owned(), "gesture".to_owned()));
                Some(made(
                    gesture,
                    token,
                    Some(config::Action::ResizeFromOpposite.label()),
                    mode.outcome(),
                ))
            }
            Action::WindowZoomBegin => {
                let (gesture, token) = self
                    .pending
                    .take()
                    .unwrap_or_else(|| ("ジェスチャー".to_owned(), "gesture".to_owned()));
                Some(made(
                    gesture,
                    token,
                    Some(config::Action::WindowZoom.label()),
                    mode.outcome(),
                ))
            }
            Action::Debug(GestureEvent::Press { fingers, deep }) => {
                self.pending = Some((
                    press_label(*fingers, *deep),
                    GestureId::press(*fingers, *deep).as_str().to_owned(),
                ));
                None
            }
            Action::Trigger(act) => {
                let (gesture, token) = self
                    .pending
                    .take()
                    .unwrap_or_else(|| ("ジェスチャー".to_owned(), "gesture".to_owned()));
                Some(made(gesture, token, Some(act.label()), mode.outcome()))
            }
            Action::Debug(GestureEvent::Tap { fingers }) => Some(made(
                format!("{fingers}本指タップ"),
                GestureId::tap(*fingers).as_str().to_owned(),
                None,
                Outcome::NoBinding,
            )),
            // The hold that has no action must not push the slider's entry off the top.
            Action::Debug(GestureEvent::LongHold { .. }) if self.edge_touch => None,
            Action::Debug(GestureEvent::LongHold { fingers }) => Some(made(
                format!("{fingers}本指で長押し"),
                GestureId::long_hold(*fingers).as_str().to_owned(),
                None,
                Outcome::NoBinding,
            )),
            Action::DragBegin => Some(made(
                "ドラッグ".to_owned(),
                "drag".to_owned(),
                Some("ドラッグ".to_owned()),
                mode.outcome(),
            )),
            Action::WindowMoveBegin => Some(made(
                "ウィンドウの移動".to_owned(),
                "window.move".to_owned(),
                Some("ウィンドウを動かす".to_owned()),
                mode.outcome(),
            )),
            Action::WindowResizeBegin => Some(made(
                "ウィンドウのサイズ変更".to_owned(),
                "window.resize".to_owned(),
                Some("ウィンドウのサイズを変える".to_owned()),
                mode.outcome(),
            )),
            _ => None,
        }
    }
}

/// Why nothing is being run right now, when the engine is not going to run gestures at all.
pub fn idle_reason(
    gestures_enabled: bool,
    paused: bool,
    app_blocked: bool,
    exe: Option<&str>,
) -> Option<String> {
    if !gestures_enabled {
        Some("ジェスチャーが無効です（ホームで有効にできます）。".to_owned())
    } else if paused {
        Some("yubi は一時停止中です。".to_owned())
    } else if app_blocked {
        Some(format!(
            "{} ではアプリ設定で yubi が無効です。",
            exe.unwrap_or("このアプリ")
        ))
    } else {
        None
    }
}

/// "たった今", "5 秒前", "3 分前".
pub fn ago_label(elapsed: Duration) -> String {
    let s = elapsed.as_secs();
    match s {
        0..=1 => "たった今".to_owned(),
        2..=59 => format!("{s} 秒前"),
        _ => format!("{} 分前", s / 60),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config;
    use crate::engine::types::MultiKind;

    fn at() -> Instant {
        Instant::now()
    }

    fn multi(fingers: usize, kind: MultiKind) -> Action {
        Action::Debug(GestureEvent::Multi { fingers, kind })
    }

    #[test]
    fn a_press_is_named_and_carries_its_action() {
        let mut r = Recorder::default();
        let mode = Mode::default();
        let press = Action::Debug(GestureEvent::Press {
            fingers: 2,
            deep: true,
        });
        assert_eq!(r.observe(&press, mode, at()), None);
        let j = r
            .observe(&Action::Trigger(config::Action::TaskView), mode, at())
            .expect("the trigger completes the judgement");
        assert_eq!(j.token, "press.2.deep");
        assert_eq!(j.gesture, press_label(2, true));
    }

    #[test]
    fn a_multi_gesture_is_named_and_carries_its_action() {
        let mut r = Recorder::default();
        let mode = Mode::default();
        assert_eq!(
            r.observe(&multi(3, MultiKind::SwipeUp), mode, at()),
            None,
            "the name waits for the trigger"
        );
        let j = r
            .observe(&Action::Trigger(config::Action::TaskView), mode, at())
            .unwrap();
        assert_eq!(j.gesture, "3本指で上にスワイプ");
        assert_eq!(j.token, "swipe.3.up");
        assert_eq!(
            j.action.as_deref(),
            Some(config::Action::TaskView.label().as_str())
        );
        assert_eq!(j.outcome, Outcome::Ran);
    }

    #[test]
    fn outcome_follows_the_dispatch_mode() {
        let trig = Action::Trigger(config::Action::ShowDesktop);
        let run = |mode| {
            let mut r = Recorder::default();
            r.observe(&trig, mode, at()).unwrap().outcome
        };
        assert_eq!(run(Mode::default()), Outcome::Ran);
        assert_eq!(
            run(Mode {
                dry_run: true,
                blocked: false
            }),
            Outcome::DryRun
        );
        // A blocked app wins over practice mode: nothing would run either way.
        assert_eq!(
            run(Mode {
                dry_run: true,
                blocked: true
            }),
            Outcome::Blocked
        );
    }

    #[test]
    fn drag_and_window_move_are_recorded_on_begin_only() {
        let mut r = Recorder::default();
        let mode = Mode::default();
        assert_eq!(
            r.observe(&Action::DragBegin, mode, at()).unwrap().token,
            "drag"
        );
        assert!(
            r.observe(
                &Action::DragMove {
                    dx_mm: 1.0,
                    dy_mm: 0.0
                },
                mode,
                at()
            )
            .is_none()
        );
        assert!(r.observe(&Action::DragEnd, mode, at()).is_none());
        assert_eq!(
            r.observe(&Action::WindowMoveBegin, mode, at())
                .unwrap()
                .token,
            "window.move"
        );
    }

    #[test]
    fn plain_taps_have_no_binding() {
        let mut r = Recorder::default();
        let j = r
            .observe(
                &Action::Debug(GestureEvent::Tap { fingers: 2 }),
                Mode::default(),
                at(),
            )
            .unwrap();
        assert_eq!((j.token.as_str(), j.outcome), ("tap.2", Outcome::NoBinding));
        assert!(j.action.is_none());
    }

    fn edge_step(side: EdgeSide, kind: EdgeKind, forward: bool) -> Action {
        Action::EdgeStep {
            side,
            kind,
            forward,
            outs: Vec::new(),
        }
    }

    #[test]
    fn an_edge_slide_is_one_entry_with_the_net_step_count() {
        let mut r = Recorder::default();
        let mut h = History::default();
        let dry = Mode {
            dry_run: true,
            blocked: false,
        };
        let mut feed = |a: Action, h: &mut History| {
            if let Some(j) = r.observe(&a, dry, at()) {
                h.push(j);
            }
        };
        for _ in 0..3 {
            feed(edge_step(EdgeSide::Right, EdgeKind::Volume, true), &mut h);
        }
        assert_eq!(h.items().len(), 1);
        let j = h.latest().unwrap();
        assert_eq!(j.gesture, "右端スライダー（音量）");
        assert_eq!(j.token, "edge.right");
        assert_eq!(j.action.as_deref(), Some("上へ +3"));
        assert_eq!(j.outcome, Outcome::DryRun);
        // Back steps count against it, and the entry keeps following the slide.
        for _ in 0..5 {
            feed(edge_step(EdgeSide::Right, EdgeKind::Volume, false), &mut h);
        }
        assert_eq!(h.items().len(), 1);
        assert_eq!(h.latest().unwrap().action.as_deref(), Some("下へ −2"));
        feed(edge_step(EdgeSide::Right, EdgeKind::Volume, true), &mut h);
        feed(edge_step(EdgeSide::Right, EdgeKind::Volume, true), &mut h);
        assert_eq!(h.latest().unwrap().action.as_deref(), Some("±0"));
        // The next slide is a new entry, and the top edge speaks of left and right.
        feed(Action::Suppress(Suppress::Off), &mut h);
        feed(edge_step(EdgeSide::Right, EdgeKind::Volume, true), &mut h);
        assert_eq!(
            h.items().len(),
            2,
            "a new slide on the same edge is its own entry"
        );
        feed(
            Action::Debug(GestureEvent::TouchEnd {
                fingers: 1,
                duration_us: 1,
            }),
            &mut h,
        );
        feed(
            edge_step(EdgeSide::Top, EdgeKind::Brightness, false),
            &mut h,
        );
        assert_eq!(h.items().len(), 3);
        let j = h.latest().unwrap();
        assert_eq!(j.gesture, "上端スライダー（明るさ）");
        assert_eq!(j.action.as_deref(), Some("左へ −1"));
    }

    #[test]
    fn a_slider_run_keeps_its_outcome_and_names_every_kind() {
        let mut r = Recorder::default();
        let blocked = Mode {
            dry_run: true,
            blocked: true,
        };
        let a = edge_step(EdgeSide::Left, EdgeKind::SeekYoutube, true);
        let j = r.observe(&a, blocked, at()).unwrap();
        assert_eq!(j.outcome, Outcome::Blocked);
        assert_eq!(j.gesture, "左端スライダー（早送り・巻き戻し）");
        for k in EdgeKind::ALL {
            assert!(!edge_kind_name(k).is_empty());
        }
    }

    #[test]
    fn a_hold_without_an_action_does_not_replace_the_slider_entry() {
        let mut r = Recorder::default();
        let mode = Mode::default();
        let hold = Action::Debug(GestureEvent::LongHold { fingers: 1 });
        // Before any slider the hold is reported as usual.
        assert!(r.observe(&hold, mode, at()).is_some());
        let step = edge_step(EdgeSide::Right, EdgeKind::Volume, true);
        assert!(r.observe(&step, mode, at()).is_some());
        // While the slider owns the touch (also after its freeze ended) the hold stays quiet.
        assert!(r.observe(&hold, mode, at()).is_none());
        r.observe(&Action::Suppress(Suppress::Off), mode, at());
        assert!(r.observe(&hold, mode, at()).is_none());
        // A new touch reports holds again.
        r.observe(&Action::Debug(GestureEvent::FingerCount(0)), mode, at());
        assert!(r.observe(&hold, mode, at()).is_some());
    }

    #[test]
    fn history_keeps_ten_newest_first_and_merges_a_bound_tap() {
        let mut r = Recorder::default();
        let mut h = History::default();
        let mode = Mode::default();
        for n in 1..=12 {
            let j = r
                .observe(
                    &Action::Debug(GestureEvent::Tap { fingers: 1 + n % 5 }),
                    mode,
                    at(),
                )
                .unwrap();
            h.push(j);
        }
        assert_eq!(h.items().len(), HISTORY_MAX);
        assert_eq!(h.latest().unwrap().token, "tap.3");
        // The unbound tap report and the bound multi tap are one gesture.
        let mut h = History::default();
        h.push(
            r.observe(&Action::Debug(GestureEvent::Tap { fingers: 3 }), mode, at())
                .unwrap(),
        );
        r.observe(&multi(3, MultiKind::Tap), mode, at());
        h.push(
            r.observe(&Action::Trigger(config::Action::Find), mode, at())
                .unwrap(),
        );
        assert_eq!(h.items().len(), 1);
        assert!(h.latest().unwrap().action.is_some());
    }

    #[test]
    fn idle_reason_names_the_cause_in_priority_order() {
        assert!(idle_reason(true, false, false, None).is_none());
        assert!(
            idle_reason(false, true, true, None)
                .unwrap()
                .contains("無効")
        );
        assert!(
            idle_reason(true, true, true, None)
                .unwrap()
                .contains("一時停止")
        );
        assert!(
            idle_reason(true, false, true, Some("game.exe"))
                .unwrap()
                .contains("game.exe")
        );
    }

    #[test]
    fn ago_labels() {
        assert_eq!(ago_label(Duration::from_millis(900)), "たった今");
        assert_eq!(ago_label(Duration::from_secs(5)), "5 秒前");
        assert_eq!(ago_label(Duration::from_secs(61)), "1 分前");
    }
}
