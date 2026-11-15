//! More animated how-to scenes, in the style of the edge and ring scenes of [`crate::howto`]: the
//! press depth bar, the long-hold ring, the combo hold (a resting finger plus a second one) and the
//! four corner triggers.
//!
//! Each scene is a pure function of a time `t` (the `*_frame` functions, testable without a window)
//! plus a painter that turns the numbers into shapes. Colours and sizes come from `Tokens`; the
//! timing comes from [`Anim`], so the scenes run, are pinned for screenshots (`howto::force`) or show
//! one still key frame when Windows' animation effects are off.

use eframe::egui::{
    Align2, Color32, FontId, Painter, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2,
};

use crate::config::{CornerTrigger, HoldTrigger};
use crate::howto::{
    Anim, PAD_MM, action_badge, arrow, ease, fade, finger, hand, pad_surface, ramp, timer_ring,
    track, trail,
};
use crate::theme::Tokens;

// ---------------------------------------------------------------------------------------------
// Press depth

/// Seconds one press demonstration lasts.
pub const PRESS_CYCLE: f32 = 4.2;
/// Where the "light" and "deep" marks sit on the depth bar (0..1).
pub const PRESS_LIGHT: f32 = 0.45;
pub const PRESS_DEEP: f32 = 0.85;
/// Time of the still frame shown when animation is off: pressed past both marks.
pub const PRESS_KEY_T: f32 = 2.9;

/// What the press scene shows at one moment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PressFrame {
    /// Finger opacity.
    pub finger: f32,
    /// How deep the press is (0..1).
    pub depth: f32,
    /// The depth is past the light / deep mark.
    pub light: bool,
    pub deep: bool,
    /// The step (0-based) of the three-step strip this moment belongs to.
    pub step: usize,
}

pub fn press_frame(time: f32) -> PressFrame {
    let l = time.max(0.0).rem_euclid(PRESS_CYCLE);
    let depth = ease(ramp(l, 0.6, 3.0)) * (1.0 - ease(ramp(l, 3.2, 3.6)));
    let deep = depth >= PRESS_DEEP;
    let light = depth >= PRESS_LIGHT;
    PressFrame {
        finger: ramp(l, 0.0, 0.3) * (1.0 - ramp(l, 3.6, 3.9)),
        depth,
        light,
        deep,
        step: if deep {
            2
        } else if light || l >= 0.6 {
            1
        } else {
            0
        },
    }
}

pub fn press_scene_width(avail: f32) -> f32 {
    avail.clamp(240.0, 400.0)
}

/// The pad with a finger pressing in, and a depth bar with the light and deep marks.
/// Returns the strip step the animation is on.
pub fn press_scene(ui: &mut Ui, t: &Tokens, anim: Anim, fingers: usize) -> usize {
    let w = press_scene_width(ui.available_width());
    let pad_w = (w * 0.46).min(190.0);
    let pad_size = Vec2::new(pad_w, pad_w * PAD_MM.1 / PAD_MM.0);
    let h = pad_size.y.max(110.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, h), Sense::hover());
    let p = ui.painter_at(rect.expand(2.0));
    let frame = press_frame(anim.at(PRESS_KEY_T));

    let pad = Rect::from_min_size(
        Pos2::new(rect.left(), rect.center().y - pad_size.y / 2.0),
        pad_size,
    );
    pad_surface(&p, t, pad);
    let dot = (pad.height() * 0.085).max(4.0);
    for h in hand(fingers) {
        let c = pad.center() + Vec2::new(h.x * pad.width() * 0.8, h.y * pad.height() * 0.8);
        // The harder the press, the wider the ring around the fingertip.
        if frame.depth > 0.0 {
            p.circle_stroke(
                c,
                dot + 2.0 + frame.depth * dot * 1.6,
                Stroke::new(
                    1.5,
                    fade(t.accent, frame.finger * (0.35 + 0.65 * frame.depth)),
                ),
            );
        }
        finger(&p, t, c, dot, frame.finger);
    }

    // Depth bar: filled from the bottom, with the two marks.
    let bar = Rect::from_min_size(
        Pos2::new(pad.right() + 22.0, pad.top()),
        Vec2::new(12.0, pad.height()),
    );
    p.rect_filled(bar, 3.0, track(t));
    let fill = Rect::from_min_max(
        Pos2::new(bar.left(), bar.bottom() - bar.height() * frame.depth),
        bar.right_bottom(),
    );
    p.rect_filled(fill, 3.0, t.accent);
    let font = FontId::proportional(t.fs_small);
    for (at, name, hit) in [
        (PRESS_DEEP, "深い押し込み", frame.deep),
        (PRESS_LIGHT, "軽い押し込み", frame.light),
    ] {
        let y = bar.bottom() - bar.height() * at;
        let color = if hit { t.text } else { t.text_muted };
        p.line_segment(
            [
                Pos2::new(bar.left() - 4.0, y),
                Pos2::new(bar.right() + 4.0, y),
            ],
            Stroke::new(1.5, if hit { t.accent } else { t.text_muted }),
        );
        p.text(
            Pos2::new(bar.right() + 10.0, y),
            Align2::LEFT_CENTER,
            name,
            font.clone(),
            color,
        );
    }
    anim.keep_alive(ui, rect);
    frame.step
}

// ---------------------------------------------------------------------------------------------
// Long hold

const HOLD_LAND: f32 = 0.5;
const HOLD_SHOW: f32 = 0.9;
const HOLD_LIFT: f32 = 0.3;
const HOLD_GAP: f32 = 0.6;

/// What the long-hold scene shows at one moment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HoldFrame {
    pub fingers: f32,
    /// Timer ring: fill (0..1) and opacity.
    pub ring: f32,
    pub ring_alpha: f32,
    /// The assigned action has run (0..1, fades out at the end of the loop).
    pub fired: f32,
    /// The step (0-based) of the three-step strip this moment belongs to.
    pub step: usize,
}

/// Seconds one long-hold demonstration lasts for a hold time of `hold_s`.
pub fn hold_period(hold_s: f32) -> f32 {
    HOLD_LAND + hold_s + HOLD_SHOW + HOLD_LIFT + HOLD_GAP
}

/// Still frame shown when animation is off: the ring is full and the action has just run.
pub fn hold_key_t(hold_s: f32) -> f32 {
    HOLD_LAND + hold_s + 0.3
}

pub fn hold_frame(time: f32, hold_s: f32) -> HoldFrame {
    let l = time.max(0.0).rem_euclid(hold_period(hold_s));
    let fire = HOLD_LAND + hold_s;
    let gone = fire + HOLD_SHOW;
    HoldFrame {
        fingers: ramp(l, 0.0, 0.3) * (1.0 - ramp(l, gone, gone + HOLD_LIFT)),
        ring: ramp(l, HOLD_LAND, fire),
        ring_alpha: ramp(l, HOLD_LAND, HOLD_LAND + 0.15) * (1.0 - ramp(l, fire, fire + 0.2)),
        fired: ramp(l, fire, fire + 0.15) * (1.0 - ramp(l, gone, gone + HOLD_LIFT)),
        step: if l < HOLD_LAND {
            0
        } else if l < fire {
            1
        } else {
            2
        },
    }
}

pub fn hold_scene_width(avail: f32) -> f32 {
    avail.clamp(240.0, 400.0)
}

/// A "screen" panel with the action badge: dim while waiting, lit when the action has run.
fn screen_with_badge(p: &Painter, t: &Tokens, panel: Rect, fired: f32, text: &str) {
    p.rect_filled(panel, t.radius_lg as f32, t.input_bg);
    p.rect_stroke(
        panel,
        t.radius_lg as f32,
        Stroke::new(t.stroke_thin, t.border),
        StrokeKind::Inside,
    );
    let size = (panel.height() * 0.46).min(panel.width() * 0.4);
    let c = panel.center() - Vec2::new(0.0, panel.height() * 0.08);
    if fired > 0.5 {
        action_badge(p, t, c, size);
    } else {
        p.rect_stroke(
            Rect::from_center_size(c, Vec2::splat(size)),
            t.radius_md as f32,
            Stroke::new(t.stroke_thin, fade(t.text_muted, 0.5)),
            StrokeKind::Inside,
        );
    }
    p.text(
        Pos2::new(panel.center().x, panel.bottom() - 10.0),
        Align2::CENTER_BOTTOM,
        text,
        FontId::proportional(t.fs_small),
        if fired > 0.5 { t.text } else { t.text_muted },
    );
}

/// The pad with `fingers` resting on it and a timer ring that fills for `hold_ms`, then the action.
/// Returns the strip step the animation is on.
pub fn hold_scene(ui: &mut Ui, t: &Tokens, anim: Anim, fingers: usize, hold_ms: u16) -> usize {
    let hold_s = f32::from(hold_ms) / 1000.0;
    let w = hold_scene_width(ui.available_width());
    let pad_w = w * 0.5;
    let pad_size = Vec2::new(pad_w, pad_w * PAD_MM.1 / PAD_MM.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, pad_size.y.max(110.0)), Sense::hover());
    let p = ui.painter_at(rect.expand(2.0));
    let frame = hold_frame(anim.at(hold_key_t(hold_s)), hold_s);

    let pad = Rect::from_min_size(
        Pos2::new(rect.left(), rect.center().y - pad_size.y / 2.0),
        pad_size,
    );
    pad_surface(&p, t, pad);
    let dot = (pad.height() * 0.085).max(4.0);
    let n = fingers.clamp(1, 5);
    for h in hand(n) {
        let c = pad.center() + Vec2::new(h.x * pad.width() * 0.8, h.y * pad.height() * 0.8);
        finger(&p, t, c, dot, frame.fingers);
    }
    if frame.ring_alpha > 0.0 {
        let extent = pad.height() * 0.2 + (n as f32 - 1.0) * pad.width() * 0.08;
        timer_ring(
            &p,
            t,
            pad.center() + Vec2::new(0.0, pad.height() * 0.06),
            extent,
            frame.ring,
            frame.ring_alpha,
        );
    }
    p.text(
        Pos2::new(pad.center().x, pad.bottom() + 4.0),
        Align2::CENTER_TOP,
        format!("{hold_ms} ms"),
        FontId::proportional(t.fs_small),
        t.text_muted,
    );
    let panel = Rect::from_min_max(
        Pos2::new(pad.right() + w * 0.08, pad.top()),
        Pos2::new(rect.right(), pad.bottom()),
    );
    screen_with_badge(&p, t, panel, frame.fired, "操作を実行");
    anim.keep_alive(ui, rect);
    frame.step
}

// ---------------------------------------------------------------------------------------------
// Combo hold (finger hold / thumb hold)

/// Seconds one combo demonstration lasts.
pub const COMBO_CYCLE: f32 = 4.4;
const COMBO_SECOND_AT: f32 = 1.3;
const COMBO_TAP_FIRE: f32 = 1.6;
const COMBO_SWIPE_FIRE: f32 = 2.3;

/// What the combo scene shows at one moment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ComboFrame {
    /// Opacity of the resting finger and of the second finger.
    pub holder: f32,
    pub second: f32,
    /// How far the second finger has slid (0..1; stays 0 for a tap).
    pub slide: f32,
    /// A tap ripple around the second finger (0..1).
    pub ripple: f32,
    /// The key combination has run.
    pub fired: f32,
    /// The step (0-based) of the three-step strip this moment belongs to.
    pub step: usize,
}

fn combo_fire(trigger: HoldTrigger) -> f32 {
    match trigger {
        HoldTrigger::Tap => COMBO_TAP_FIRE,
        HoldTrigger::Swipe => COMBO_SWIPE_FIRE,
    }
}

/// Still frame shown when animation is off: just after the combination ran.
pub fn combo_key_t(trigger: HoldTrigger) -> f32 {
    combo_fire(trigger) + 0.3
}

pub fn combo_frame(time: f32, trigger: HoldTrigger) -> ComboFrame {
    let l = time.max(0.0).rem_euclid(COMBO_CYCLE);
    let fire = combo_fire(trigger);
    let (second_gone, slide, ripple) = match trigger {
        HoldTrigger::Tap => (
            COMBO_TAP_FIRE + 0.1,
            0.0,
            ramp(l, COMBO_SECOND_AT + 0.1, COMBO_TAP_FIRE + 0.6),
        ),
        HoldTrigger::Swipe => (
            COMBO_SWIPE_FIRE + 0.5,
            ease(ramp(l, COMBO_SECOND_AT + 0.3, COMBO_SWIPE_FIRE)),
            0.0,
        ),
    };
    let gone = fire + 1.4;
    ComboFrame {
        holder: ramp(l, 0.0, 0.3) * (1.0 - ramp(l, gone, gone + 0.3)),
        second: ramp(l, COMBO_SECOND_AT, COMBO_SECOND_AT + 0.2)
            * (1.0 - ramp(l, second_gone, second_gone + 0.2)),
        slide,
        ripple,
        fired: ramp(l, fire, fire + 0.15) * (1.0 - ramp(l, gone, gone + 0.3)),
        step: if l < COMBO_SECOND_AT {
            0
        } else if l < fire {
            1
        } else {
            2
        },
    }
}

pub fn combo_scene_width(avail: f32) -> f32 {
    avail.clamp(240.0, 400.0)
}

/// The pad with one resting finger (the thumb when `thumb`) and a second finger that taps or swipes,
/// then the key combination `keys`. Returns the strip step the animation is on.
pub fn combo_scene(
    ui: &mut Ui,
    t: &Tokens,
    anim: Anim,
    thumb: bool,
    trigger: HoldTrigger,
    keys: &str,
) -> usize {
    let w = combo_scene_width(ui.available_width());
    let pad_w = w * 0.5;
    let pad_size = Vec2::new(pad_w, pad_w * PAD_MM.1 / PAD_MM.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, pad_size.y.max(110.0)), Sense::hover());
    let p = ui.painter_at(rect.expand(2.0));
    let frame = combo_frame(anim.at(combo_key_t(trigger)), trigger);

    let pad = Rect::from_min_size(
        Pos2::new(rect.left(), rect.center().y - pad_size.y / 2.0),
        pad_size,
    );
    pad_surface(&p, t, pad);
    let dot = (pad.height() * 0.085).max(4.0);
    let (hold_at, other_at) = crate::howto::combo_spots(pad, thumb);
    finger(&p, t, hold_at, dot, frame.holder);
    p.circle_stroke(
        hold_at,
        dot + 4.0,
        Stroke::new(1.3, fade(t.accent, 0.6 * frame.holder)),
    );
    let slide_to = other_at + Vec2::new(pad.width() * 0.22, 0.0);
    let pos = other_at.lerp(slide_to, frame.slide);
    if frame.slide > 0.0 {
        trail(&p, t, other_at, pos, 3.0, frame.second);
    }
    finger(&p, t, pos, dot, frame.second);
    if frame.ripple > 0.0 && frame.ripple < 1.0 {
        p.circle_stroke(
            other_at,
            dot + 2.0 + frame.ripple * dot * 2.5,
            Stroke::new(1.5, fade(t.accent, 1.0 - frame.ripple)),
        );
    }
    let name = if thumb { "親指" } else { "指" };
    p.text(
        hold_at + Vec2::new(0.0, dot + 7.0),
        Align2::CENTER_TOP,
        format!("{name}を置いたまま"),
        FontId::proportional(t.fs_small - 1.0),
        fade(t.text_muted, frame.holder),
    );
    let panel = Rect::from_min_max(
        Pos2::new(pad.right() + w * 0.08, pad.top()),
        Pos2::new(rect.right(), pad.bottom()),
    );
    let shown = if keys.trim().is_empty() {
        "キー入力"
    } else {
        keys.trim()
    };
    screen_with_badge(&p, t, panel, frame.fired, shown);
    anim.keep_alive(ui, rect);
    frame.step
}

// ---------------------------------------------------------------------------------------------
// Corner triggers

/// Seconds one trigger's demonstration lasts.
pub const CORNER_CYCLE: f32 = 3.4;

/// The four captions, in the order of [`CornerTrigger::ALL`].
pub const CORNER_CAPTIONS: [&str; 4] = [
    "タップ: 触れてすぐ離す",
    "スワイプ: 角から外へなぞる",
    "プレス: 角で押し込む",
    "ホールド: 角で指を止める",
];

/// Index of `trigger` in [`CornerTrigger::ALL`].
pub fn trigger_index(trigger: CornerTrigger) -> usize {
    match trigger {
        CornerTrigger::Tap => 0,
        CornerTrigger::Swipe => 1,
        CornerTrigger::Press => 2,
        CornerTrigger::Hold => 3,
    }
}

/// What the corner scene shows at one moment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CornerFrame {
    pub trigger: CornerTrigger,
    pub finger: f32,
    /// How far the finger has moved out of the corner (swipe, 0..1).
    pub slide: f32,
    /// Timer ring fill and opacity (hold).
    pub ring: f32,
    pub ring_alpha: f32,
    /// Tap ripple / press depth (0..1).
    pub pulse: f32,
    /// The assigned action has run (0..1).
    pub fired: f32,
    /// 0: placing the finger, 1: doing the trigger, 2: the action ran.
    pub stage: usize,
}

/// (time the finger lifts, time the action fires) for each trigger.
fn corner_times(trigger: CornerTrigger) -> (f32, f32) {
    match trigger {
        CornerTrigger::Tap => (0.6, 0.6),
        CornerTrigger::Swipe => (2.2, 1.7),
        CornerTrigger::Press => (2.0, 1.3),
        CornerTrigger::Hold => (2.5, 2.0),
    }
}

/// Still frame shown when animation is off: the trigger has just fired.
pub fn corner_key_t(trigger: CornerTrigger, only: bool) -> f32 {
    let at = corner_times(trigger).1 + 0.2;
    if only {
        at
    } else {
        trigger_index(trigger) as f32 * CORNER_CYCLE + at
    }
}

/// The corner demonstration at `time`: with `only`, that trigger on a loop; otherwise the four in
/// turn.
pub fn corner_frame(time: f32, only: Option<CornerTrigger>) -> CornerFrame {
    let time = time.max(0.0);
    let trigger = only.unwrap_or_else(|| {
        CornerTrigger::ALL[(time / CORNER_CYCLE) as usize % CornerTrigger::ALL.len()]
    });
    let l = time.rem_euclid(CORNER_CYCLE);
    let (lift, fire) = corner_times(trigger);
    let (slide, ring, pulse) = match trigger {
        CornerTrigger::Tap => (0.0, 0.0, ramp(l, 0.3, 1.0)),
        CornerTrigger::Swipe => (ease(ramp(l, 0.7, 1.7)), 0.0, 0.0),
        CornerTrigger::Press => (0.0, 0.0, ease(ramp(l, 0.9, 1.3))),
        CornerTrigger::Hold => (0.0, ramp(l, 0.5, 2.0), 0.0),
    };
    let ring_alpha = if trigger == CornerTrigger::Hold {
        ramp(l, 0.5, 0.65) * (1.0 - ramp(l, 2.0, 2.2))
    } else {
        0.0
    };
    CornerFrame {
        trigger,
        finger: ramp(l, 0.2, 0.4) * (1.0 - ramp(l, lift, lift + 0.2)),
        slide,
        ring,
        ring_alpha,
        pulse,
        fired: ramp(l, fire, fire + 0.15) * (1.0 - ramp(l, CORNER_CYCLE - 0.5, CORNER_CYCLE - 0.2)),
        stage: if l < 0.4 {
            0
        } else if l < fire {
            1
        } else {
            2
        },
    }
}

/// Width the corner scene takes.
pub fn corner_scene_width(avail: f32) -> f32 {
    avail.clamp(220.0, 480.0)
}

/// Whether the captions sit beside the pad (otherwise below it).
fn captions_beside(w: f32) -> bool {
    w >= 400.0
}

fn caption_dot(p: &Painter, t: &Tokens, c: Pos2, active: bool) {
    p.circle_filled(c, 3.5, if active { t.accent } else { t.border });
}

/// The pad with the bottom-left corner zone highlighted, the trigger(s) played on it and the four
/// captions. `only` loops one trigger (its caption is highlighted); `None` plays the four in turn.
/// Returns the stage (see [`CornerFrame::stage`]) the animation is on.
pub fn corner_scene(
    ui: &mut Ui,
    t: &Tokens,
    anim: Anim,
    only: Option<CornerTrigger>,
    size_mm: f32,
) -> usize {
    let w = corner_scene_width(ui.available_width());
    let beside = captions_beside(w);
    let pad_w = if beside {
        (w * 0.5).min(230.0)
    } else {
        w.min(260.0)
    };
    let pad_size = Vec2::new(pad_w, pad_w * PAD_MM.1 / PAD_MM.0);
    let font = FontId::proportional(t.fs_small);
    let cap_w = if beside { w - pad_w - 20.0 } else { w - 16.0 };
    let galleys: Vec<_> = {
        let ui_ref: &Ui = ui;
        CORNER_CAPTIONS
            .iter()
            .map(|c| {
                ui_ref
                    .painter()
                    .layout((*c).to_owned(), font.clone(), t.text, cap_w.max(60.0))
            })
            .collect()
    };
    let cap_h: f32 = galleys.iter().map(|g| g.size().y + 6.0).sum();
    let h = if beside {
        pad_size.y.max(cap_h)
    } else {
        pad_size.y + 10.0 + cap_h
    };
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, h), Sense::hover());
    let p = ui.painter_at(rect.expand(2.0));
    let key = only.map_or(corner_key_t(CornerTrigger::Swipe, false), |tr| {
        corner_key_t(tr, true)
    });
    let frame = corner_frame(anim.at(key), only);

    let pad = Rect::from_min_size(
        Pos2::new(
            rect.left(),
            if beside {
                rect.center().y - pad_size.y / 2.0
            } else {
                rect.top()
            },
        ),
        pad_size,
    );
    pad_surface(&p, t, pad);
    let scale = pad.width() / PAD_MM.0;
    let zone_side = (size_mm * scale).clamp(24.0, pad.width() * 0.3);
    let zone = Rect::from_min_size(
        Pos2::new(pad.left(), pad.bottom() - zone_side),
        Vec2::splat(zone_side),
    );
    p.rect_filled(zone, t.radius_sm as f32, t.accent_soft);
    p.rect_stroke(
        zone,
        t.radius_sm as f32,
        Stroke::new(t.stroke_thin, t.accent),
        StrokeKind::Inside,
    );
    let dot = (pad.height() * 0.07).max(4.0);
    let start = zone.center();
    let end = start + (pad.center() - start) * 0.9;
    let pos = start.lerp(end, frame.slide);
    match frame.trigger {
        CornerTrigger::Tap => {
            if frame.pulse > 0.0 && frame.pulse < 1.0 {
                p.circle_stroke(
                    start,
                    dot + 2.0 + frame.pulse * dot * 2.6,
                    Stroke::new(1.5, fade(t.accent, 1.0 - frame.pulse)),
                );
            }
        }
        CornerTrigger::Swipe => {
            if frame.slide > 0.0 {
                trail(&p, t, start, pos, 3.0, frame.finger);
            }
            arrow(
                &p,
                start.lerp(end, 0.1),
                end,
                Stroke::new(1.0, fade(t.accent, 0.35)),
            );
        }
        CornerTrigger::Press => {
            for (k, base) in [(0.0f32, 3.0f32), (0.55, 7.0)] {
                let a = ramp(frame.pulse, k, k + 0.45);
                if a > 0.0 {
                    p.circle_stroke(
                        start,
                        dot + base + a * 2.0,
                        Stroke::new(1.5, fade(t.accent, frame.finger * (0.4 + 0.6 * a))),
                    );
                }
            }
        }
        CornerTrigger::Hold => {
            if frame.ring_alpha > 0.0 {
                timer_ring(&p, t, start, dot + 6.0, frame.ring, frame.ring_alpha);
            }
        }
    }
    finger(&p, t, pos, dot, frame.finger);
    if frame.fired > 0.5 {
        action_badge(
            &p,
            t,
            pad.center() + Vec2::new(pad.width() * 0.2, -pad.height() * 0.12),
            pad.height() * 0.3,
        );
    }

    // Captions: the one being played is lit.
    let (mut x, mut y) = if beside {
        (pad.right() + 20.0, rect.center().y - cap_h / 2.0)
    } else {
        (rect.left() + 12.0, pad.bottom() + 10.0)
    };
    let current = trigger_index(frame.trigger);
    let live = anim.live();
    for (i, g) in galleys.into_iter().enumerate() {
        let active = i == current && (live || only.is_some());
        let color: Color32 = if active { t.text } else { t.text_muted };
        let row_h = g.size().y;
        caption_dot(&p, t, Pos2::new(x - 9.0, y + row_h / 2.0), active);
        p.galley(Pos2::new(x, y), g, color);
        y += row_h + 6.0;
        if !beside {
            x = rect.left() + 12.0;
        }
    }
    anim.keep_alive(ui, rect);
    frame.stage
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn press_deepens_past_light_then_deep_then_lets_go() {
        let f = press_frame(0.1);
        assert_eq!((f.step, f.depth), (0, 0.0));
        assert!(f.finger > 0.0 && f.finger < 1.0);
        let f = press_frame(1.4);
        assert_eq!(f.step, 1);
        assert!(f.depth > 0.0 && !f.deep);
        let f = press_frame(PRESS_KEY_T);
        assert!(f.light && f.deep && f.depth > PRESS_DEEP);
        assert_eq!(f.step, 2);
        let f = press_frame(PRESS_CYCLE - 0.05);
        assert_eq!((f.depth, f.finger), (0.0, 0.0));
        assert_eq!(press_frame(PRESS_CYCLE + 0.1).step, 0, "loops");
    }

    #[test]
    fn press_marks_are_ordered() {
        const { assert!(0.0 < PRESS_LIGHT && PRESS_LIGHT < PRESS_DEEP && PRESS_DEEP < 1.0) };
    }

    #[test]
    fn hold_ring_fills_for_exactly_the_hold_time() {
        let hold = 0.5;
        let f = hold_frame(0.1, hold);
        assert_eq!((f.step, f.ring), (0, 0.0));
        let f = hold_frame(HOLD_LAND + 0.25, hold);
        assert_eq!(f.step, 1);
        assert!((f.ring - 0.5).abs() < 1e-5 && f.ring_alpha > 0.0);
        assert_eq!(f.fired, 0.0);
        let f = hold_frame(HOLD_LAND + hold - 0.01, hold);
        assert_eq!((f.step, f.fired), (1, 0.0));
        let f = hold_frame(hold_key_t(hold), hold);
        assert_eq!(f.step, 2);
        assert_eq!(f.ring, 1.0);
        assert_eq!(f.fired, 1.0);
        assert_eq!(hold_frame(hold_period(hold) + 0.1, hold).step, 0, "loops");
    }

    #[test]
    fn hold_period_grows_with_the_hold_time() {
        assert!((hold_period(1.0) - hold_period(0.5) - 0.5).abs() < 1e-5);
    }

    #[test]
    fn combo_tap_and_swipe_fire_at_different_times() {
        for trigger in [HoldTrigger::Tap, HoldTrigger::Swipe] {
            let f = combo_frame(0.1, trigger);
            assert_eq!((f.step, f.second, f.fired), (0, 0.0, 0.0));
            let f = combo_frame(COMBO_SECOND_AT + 0.1, trigger);
            assert_eq!(f.step, 1);
            assert!(f.second > 0.0 && f.holder == 1.0);
            let f = combo_frame(combo_key_t(trigger), trigger);
            assert_eq!(f.step, 2);
            assert_eq!(f.fired, 1.0);
        }
        assert!(combo_fire(HoldTrigger::Swipe) > combo_fire(HoldTrigger::Tap));
        // Only the swipe slides; only the tap ripples.
        assert!(combo_frame(2.0, HoldTrigger::Swipe).slide > 0.0);
        assert_eq!(combo_frame(2.0, HoldTrigger::Tap).slide, 0.0);
        assert!(combo_frame(1.6, HoldTrigger::Tap).ripple > 0.0);
        assert_eq!(combo_frame(1.6, HoldTrigger::Swipe).ripple, 0.0);
    }

    #[test]
    fn corner_scene_plays_the_four_triggers_in_turn() {
        for (i, tr) in CornerTrigger::ALL.into_iter().enumerate() {
            let f = corner_frame(i as f32 * CORNER_CYCLE + 0.1, None);
            assert_eq!(f.trigger, tr);
            assert_eq!(f.stage, 0);
            assert_eq!(trigger_index(tr), i);
        }
        assert_eq!(
            corner_frame(4.0 * CORNER_CYCLE + 0.1, None).trigger,
            CornerTrigger::Tap,
            "loops"
        );
    }

    #[test]
    fn corner_key_frames_show_each_trigger_fired() {
        for tr in CornerTrigger::ALL {
            let f = corner_frame(corner_key_t(tr, true), Some(tr));
            assert_eq!((f.trigger, f.stage, f.fired), (tr, 2, 1.0), "{tr:?}");
            let f = corner_frame(corner_key_t(tr, false), None);
            assert_eq!((f.trigger, f.fired), (tr, 1.0), "{tr:?} in turn");
        }
    }

    #[test]
    fn corner_triggers_move_differently() {
        let f = corner_frame(1.2, Some(CornerTrigger::Swipe));
        assert!(f.slide > 0.0 && f.slide < 1.0 && f.finger == 1.0);
        let f = corner_frame(1.2, Some(CornerTrigger::Hold));
        assert!(f.ring > 0.0 && f.ring < 1.0 && f.ring_alpha > 0.0);
        assert_eq!(f.slide, 0.0);
        let f = corner_frame(1.1, Some(CornerTrigger::Press));
        assert!(f.pulse > 0.0 && f.pulse < 1.0);
        // A tap is over almost at once.
        let f = corner_frame(0.5, Some(CornerTrigger::Tap));
        assert!(f.finger > 0.0 && f.stage == 1);
        let f = corner_frame(0.9, Some(CornerTrigger::Tap));
        assert_eq!(f.finger, 0.0);
        assert_eq!(f.stage, 2);
    }

    #[test]
    fn captions_cover_every_trigger_in_order() {
        assert_eq!(CORNER_CAPTIONS.len(), CornerTrigger::ALL.len());
        for (cap, tr) in CORNER_CAPTIONS.iter().zip(CornerTrigger::ALL) {
            assert!(cap.starts_with(tr.label()), "{cap}");
        }
    }
}
