//! How-to illustrations: a strip of small step cards (mini trackpad, finger dots, chevrons, a caption
//! under each) and animated trackpad scenes for the edge slider and the ring menu.
//!
//! The scenes are pure functions of a time `t`: the timing helpers at the top are testable without a
//! window, the painters below only turn those numbers into shapes. Colours, radii and sizes come from
//! `Tokens`. The animation repaints (~30 fps) only while a scene is on screen, and shows one static key
//! frame when Windows' "animation effects" are off.

use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::time::Duration;

use eframe::egui::{
    self, Align2, Color32, FontId, Painter, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Ui, Vec2,
};

use crate::icons;
use crate::theme::Tokens;
use crate::widgets::paint_icon;

// ---------------------------------------------------------------------------------------------
// Timing (pure)

/// 0 before `a`, 1 after `b`, linear in between.
pub fn ramp(t: f32, a: f32, b: f32) -> f32 {
    if b <= a {
        return if t >= b { 1.0 } else { 0.0 };
    }
    ((t - a) / (b - a)).clamp(0.0, 1.0)
}

/// Smoothstep.
pub fn ease(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

fn lerp(a: f32, b: f32, u: f32) -> f32 {
    a + (b - a) * u
}

fn fade(c: Color32, a: f32) -> Color32 {
    c.gamma_multiply(a.clamp(0.0, 1.0))
}

/// Seconds one side's demonstration lasts.
pub const EDGE_CYCLE: f32 = 4.0;
/// Segments of the mini level bar.
pub const EDGE_STEPS: usize = 6;
/// Time (in the first side's cycle) of the still frame shown when animation is off.
pub const EDGE_KEY_T: f32 = 2.0;

/// What the edge scene shows at one moment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EdgeFrame {
    /// Which of the shown sides is being demonstrated.
    pub side: usize,
    /// Finger opacity.
    pub finger: f32,
    /// Dwell ring: fill (0..1) and opacity.
    pub dwell: f32,
    pub dwell_alpha: f32,
    /// Position along the edge (0 = start, 1 = end).
    pub slide: f32,
    /// Filled segments of the level bar and its opacity.
    pub level: usize,
    pub level_alpha: f32,
    /// The step (0-based) of the three-step strip this moment belongs to.
    pub step: usize,
}

/// The edge demonstration at `time`: for each side in turn, a finger lands in the band, waits, slides
/// along the edge (the level bar fills step by step), rests, and lifts.
pub fn edge_frame(time: f32, sides: usize) -> EdgeFrame {
    let sides = sides.max(1);
    let c = time.rem_euclid(EDGE_CYCLE * sides as f32);
    let side = ((c / EDGE_CYCLE) as usize).min(sides - 1);
    let l = c - side as f32 * EDGE_CYCLE;
    let finger = ramp(l, 0.0, 0.3) * (1.0 - ramp(l, 3.5, 3.85));
    let slide = ease(ramp(l, 0.8, 3.0));
    let level = ((slide * EDGE_STEPS as f32) as usize).min(EDGE_STEPS);
    let step = if l < 0.3 {
        0
    } else if l < 0.8 {
        1
    } else {
        2
    };
    EdgeFrame {
        side,
        finger,
        dwell: ramp(l, 0.3, 0.6),
        dwell_alpha: ramp(l, 0.3, 0.4) * (1.0 - ramp(l, 0.6, 0.85)),
        slide,
        level,
        level_alpha: ramp(l, 0.7, 1.0) * (1.0 - ramp(l, 3.5, 3.9)),
        step,
    }
}

/// What the ring scene shows at one moment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RingFrame {
    /// Which loop this is (the slot picked advances each loop).
    pub cycle: usize,
    /// Opacity of the fingers on the pad.
    pub fingers: f32,
    /// Hold timer ring: fill (0..1) and opacity.
    pub hold: f32,
    pub hold_alpha: f32,
    /// How far the ring has opened (0..1).
    pub open: f32,
    /// How far the hand has moved toward the slot (0..1).
    pub moved: f32,
    /// The target slot is highlighted.
    pub selected: bool,
    /// The chosen item flashes "run" (0..1) and the ring fades out afterwards.
    pub flash: f32,
    pub fade_out: f32,
    /// The step (0-based) of the four-step strip this moment belongs to.
    pub step: usize,
}

const RING_LAND: f32 = 0.5;
const RING_OPEN: f32 = 0.25;
const RING_MOVE_AT: f32 = 0.35;
const RING_MOVE: f32 = 0.8;
const RING_REST: f32 = 0.4;
const RING_LIFT: f32 = 0.25;
const RING_FLASH: f32 = 0.9;
const RING_CLOSE: f32 = 0.3;
const RING_GAP: f32 = 0.5;

/// Seconds one ring demonstration lasts for a hold time of `hold_s`.
pub fn ring_period(hold_s: f32) -> f32 {
    RING_LAND
        + hold_s
        + RING_MOVE_AT
        + RING_MOVE
        + RING_REST
        + RING_LIFT
        + RING_FLASH
        + RING_CLOSE
        + RING_GAP
}

/// Time of the still frame shown when animation is off: the hand has moved, the slot is highlighted.
pub fn ring_key_t(hold_s: f32) -> f32 {
    RING_LAND + hold_s + RING_MOVE_AT + RING_MOVE + 0.1
}

pub fn ring_frame(time: f32, hold_s: f32) -> RingFrame {
    let period = ring_period(hold_s);
    let time = time.max(0.0);
    let cycle = (time / period) as usize;
    let l = time - cycle as f32 * period;
    let open_at = RING_LAND + hold_s;
    let move_at = open_at + RING_MOVE_AT;
    let lift_at = move_at + RING_MOVE + RING_REST;
    let flash_at = lift_at + RING_LIFT;
    let moved = ease(ramp(l, move_at, move_at + RING_MOVE));
    let step = if l < RING_LAND {
        0
    } else if l < open_at {
        1
    } else if l < lift_at {
        2
    } else {
        3
    };
    RingFrame {
        cycle,
        fingers: ramp(l, 0.0, RING_LAND) * (1.0 - ramp(l, lift_at, flash_at)),
        hold: ramp(l, RING_LAND, open_at),
        hold_alpha: ramp(l, RING_LAND, RING_LAND + 0.15) * (1.0 - ramp(l, open_at, open_at + 0.2)),
        open: ease(ramp(l, open_at, open_at + RING_OPEN))
            * (1.0 - ramp(l, flash_at + RING_FLASH, flash_at + RING_FLASH + RING_CLOSE)),
        moved,
        selected: moved > 0.35,
        flash: ramp(l, flash_at, flash_at + 0.15),
        fade_out: ramp(l, flash_at + RING_FLASH, flash_at + RING_FLASH + RING_CLOSE),
        step,
    }
}

// ---------------------------------------------------------------------------------------------
// Layout (pure)

pub const CARD_W_MAX: f32 = 104.0;
const CARD_W_MIN: f32 = 72.0;
const CARD_W_COLUMN: f32 = 88.0;
const CHEV_W: f32 = 24.0;
const COL_GAP: f32 = 24.0;
/// Narrowest text column that is worth putting beside an illustration.
const MIN_TEXT_W: f32 = 240.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StripLayout {
    /// Cards stacked in a column with the caption beside each, instead of a row with chevrons.
    pub vertical: bool,
    pub card_w: f32,
}

/// Row of `n` cards when it fits `avail` (cards shrink down to a minimum), a column otherwise.
pub fn strip_layout(avail: f32, n: usize) -> StripLayout {
    let n = n.max(1);
    let w = ((avail - CHEV_W * (n - 1) as f32) / n as f32).min(CARD_W_MAX);
    if w < CARD_W_MIN {
        StripLayout {
            vertical: true,
            card_w: CARD_W_COLUMN,
        }
    } else {
        StripLayout {
            vertical: false,
            card_w: w,
        }
    }
}

/// Width of a row of `n` cards.
pub fn strip_width(n: usize, card_w: f32) -> f32 {
    n as f32 * card_w + CHEV_W * n.saturating_sub(1) as f32
}

/// Whether an illustration `left_w` wide fits beside a text column in `avail`.
pub fn side_by_side(avail: f32, left_w: f32) -> bool {
    avail >= left_w + COL_GAP + MIN_TEXT_W
}

// ---------------------------------------------------------------------------------------------
// Motion

static FORCE_T_MS: AtomicI32 = AtomicI32::new(-1);
static FORCE_REDUCED: AtomicBool = AtomicBool::new(false);

/// Development aid (`YUBI_SHOT`): pin the scenes to time `t` seconds, and/or behave as if Windows'
/// animation effects were off.
pub fn force(t: Option<f32>, reduced: bool) {
    FORCE_T_MS.store(t.map_or(-1, |t| (t * 1000.0) as i32), Ordering::Relaxed);
    FORCE_REDUCED.store(reduced, Ordering::Relaxed);
}

#[cfg(windows)]
fn os_animations() -> bool {
    use std::ffi::c_void;
    use windows::Win32::UI::WindowsAndMessaging::{
        SPI_GETCLIENTAREAANIMATION, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW,
    };
    let mut on: i32 = 1;
    // SAFETY: SPI_GETCLIENTAREAANIMATION writes one BOOL.
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            Some(&mut on as *mut i32 as *mut c_void),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    ok.is_err() || on != 0
}

#[cfg(not(windows))]
fn os_animations() -> bool {
    true
}

/// How a scene is timed this frame.
#[derive(Debug, Clone, Copy)]
pub struct Anim {
    now: f32,
    /// Time is pinned (screenshots).
    forced: bool,
    /// Animation is allowed (Windows' animation effects are on).
    moving: bool,
}

impl Anim {
    pub fn new(ui: &Ui) -> Self {
        let pinned = FORCE_T_MS.load(Ordering::Relaxed);
        let reduced = FORCE_REDUCED.load(Ordering::Relaxed);
        Self {
            now: if pinned >= 0 {
                pinned as f32 / 1000.0
            } else {
                ui.input(|i| i.time) as f32
            },
            forced: pinned >= 0,
            moving: pinned < 0 && !reduced && os_animations(),
        }
    }

    /// The time a scene draws: the running clock, the pinned time, or the still `key` frame.
    pub fn at(&self, key: f32) -> f32 {
        if self.moving || self.forced {
            self.now
        } else {
            key
        }
    }

    /// The scene runs (or is pinned), so the step it is on is worth highlighting.
    pub fn live(&self) -> bool {
        self.moving || self.forced
    }

    /// Ask for the next frame, but only while the scene at `rect` is really on screen.
    fn keep_alive(&self, ui: &Ui, rect: Rect) {
        let (minimized, hidden) = ui.ctx().input(|i| {
            let v = i.viewport();
            (v.minimized == Some(true), v.visible() == Some(false))
        });
        if self.moving && !minimized && !hidden && ui.is_rect_visible(rect) {
            ui.ctx().request_repaint_after(Duration::from_millis(33));
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Drawing helpers

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
    Top,
}

impl Side {
    pub fn name(self) -> &'static str {
        match self {
            Self::Left => "左端",
            Self::Right => "右端",
            Self::Top => "上端",
        }
    }
}

/// Where fingers rest on the pad (unit square, centred): a gentle arc like a relaxed hand.
fn hand(n: usize) -> Vec<Vec2> {
    let n = n.clamp(1, 5);
    (0..n)
        .map(|i| {
            let x = (i as f32 - (n as f32 - 1.0) / 2.0) * 0.30;
            Vec2::new(x, -0.12 + x * x * 1.1)
        })
        .collect()
}

fn arc(p: &Painter, c: Pos2, r: f32, a0: f32, a1: f32, stroke: Stroke) {
    let n = (((a1 - a0).abs() * r / 3.0).ceil() as usize).clamp(2, 64);
    let pts = (0..=n)
        .map(|i| c + Vec2::angled(lerp(a0, a1, i as f32 / n as f32)) * r)
        .collect();
    p.add(Shape::line(pts, stroke));
}

fn dashed_ring(p: &Painter, c: Pos2, r: f32, stroke: Stroke) {
    const DASHES: usize = 10;
    let step = std::f32::consts::TAU / DASHES as f32;
    for i in 0..DASHES {
        let a = i as f32 * step;
        arc(p, c, r, a, a + step * 0.55, stroke);
    }
}

/// A timer ring: faint track plus the filled part, starting at 12 o'clock.
fn timer_ring(p: &Painter, t: &Tokens, c: Pos2, r: f32, frac: f32, alpha: f32) {
    p.circle_stroke(c, r, Stroke::new(1.5, fade(t.accent, 0.25 * alpha)));
    if frac > 0.0 {
        let a0 = -std::f32::consts::FRAC_PI_2;
        arc(
            p,
            c,
            r,
            a0,
            a0 + std::f32::consts::TAU * frac,
            Stroke::new(2.0, fade(t.accent, alpha)),
        );
    }
}

/// Finger dot shadow and highlight strength (0..255 before fading).
const SHADOW_ALPHA: f32 = 70.0;
const SHEEN_ALPHA: f32 = 80.0;

fn finger(p: &Painter, t: &Tokens, c: Pos2, r: f32, alpha: f32) {
    if alpha <= 0.0 {
        return;
    }
    p.circle_filled(
        c + Vec2::new(0.0, r * 0.35),
        r * 1.15,
        Color32::from_black_alpha((SHADOW_ALPHA * alpha) as u8),
    );
    p.circle_filled(c, r, fade(t.accent, alpha));
    p.circle_filled(
        c - Vec2::splat(r * 0.3),
        r * 0.35,
        Color32::from_white_alpha((SHEEN_ALPHA * alpha) as u8),
    );
}

fn arrow(p: &Painter, from: Pos2, to: Pos2, stroke: Stroke) {
    p.line_segment([from, to], stroke);
    let d = (to - from).normalized();
    let n = Vec2::new(-d.y, d.x);
    let head = 4.5;
    p.line_segment([to, to - d * head + n * head * 0.6], stroke);
    p.line_segment([to, to - d * head - n * head * 0.6], stroke);
}

fn clock_badge(p: &Painter, t: &Tokens, c: Pos2) {
    p.circle_filled(c, 7.0, t.accent);
    let s = Stroke::new(1.3, t.on_accent);
    p.line_segment([c, c + Vec2::new(0.0, -4.0)], s);
    p.line_segment([c, c + Vec2::new(2.8, 1.2)], s);
}

/// A small rounded square with a four-point sparkle: "the assigned action runs".
fn action_badge(p: &Painter, t: &Tokens, c: Pos2, size: f32) {
    let r = Rect::from_center_size(c, Vec2::splat(size));
    p.rect_filled(r, t.radius_md as f32, t.accent_soft);
    p.rect_stroke(
        r,
        t.radius_md as f32,
        Stroke::new(t.stroke_thin, t.accent),
        StrokeKind::Inside,
    );
    let (long, short) = (size * 0.32, size * 0.07);
    for (w, h) in [(short, long), (long, short)] {
        p.add(Shape::convex_polygon(
            vec![
                c + Vec2::new(0.0, -h),
                c + Vec2::new(w, 0.0),
                c + Vec2::new(0.0, h),
                c + Vec2::new(-w, 0.0),
            ],
            t.accent,
            Stroke::NONE,
        ));
    }
}

fn chevron(p: &Painter, t: &Tokens, c: Pos2) {
    let s = Stroke::new(1.5, t.text_muted);
    p.line_segment([c + Vec2::new(-2.5, -5.0), c + Vec2::new(2.5, 0.0)], s);
    p.line_segment([c + Vec2::new(2.5, 0.0), c + Vec2::new(-2.5, 5.0)], s);
}

fn circled_number(p: &Painter, t: &Tokens, c: Pos2, n: usize, active: bool) {
    let (fill, text) = if active {
        (t.accent, t.on_accent)
    } else {
        (t.control_hover, t.text)
    };
    p.circle_filled(c, 7.5, fill);
    p.text(
        c,
        Align2::CENTER_CENTER,
        n.to_string(),
        FontId::proportional(t.fs_small - 2.0),
        text,
    );
}

/// The light theme has a light page; its surfaces need a little more tint to show up.
fn is_light(t: &Tokens) -> bool {
    t.bg.r() > 128
}

/// An empty segment of the level bar.
fn track(t: &Tokens) -> Color32 {
    if is_light(t) { t.border } else { t.control }
}

fn pad_surface(p: &Painter, t: &Tokens, pad: Rect) {
    let fill = if is_light(t) {
        t.nav_selected
    } else {
        t.control
    };
    p.rect_filled(pad, t.radius_sm as f32 + 2.0, fill);
    p.rect_stroke(
        pad,
        t.radius_sm as f32 + 2.0,
        Stroke::new(t.stroke_thin, t.border),
        StrokeKind::Inside,
    );
}

fn band_rect(pad: Rect, side: Side, thick: f32) -> Rect {
    match side {
        Side::Left => Rect::from_min_size(pad.min, Vec2::new(thick, pad.height())),
        Side::Right => Rect::from_min_size(
            Pos2::new(pad.right() - thick, pad.top()),
            Vec2::new(thick, pad.height()),
        ),
        Side::Top => Rect::from_min_size(pad.min, Vec2::new(pad.width(), thick)),
    }
}

fn paint_band(p: &Painter, t: &Tokens, r: Rect, strong: bool) {
    p.rect_filled(
        r,
        0.0,
        if strong {
            t.accent_soft
        } else {
            fade(t.accent_soft, 0.6)
        },
    );
    p.rect_stroke(
        r,
        0.0,
        Stroke::new(
            t.stroke_thin,
            fade(t.accent, if strong { 1.0 } else { 0.5 }),
        ),
        StrokeKind::Inside,
    );
}

/// The finger's position `u` (0..1) of the way along `side`'s band.
fn edge_point(pad: Rect, side: Side, thick: f32, u: f32) -> Pos2 {
    match side {
        Side::Left | Side::Right => {
            let x = if side == Side::Left {
                pad.left() + thick / 2.0
            } else {
                pad.right() - thick / 2.0
            };
            Pos2::new(
                x,
                lerp(
                    pad.bottom() - pad.height() * 0.16,
                    pad.top() + pad.height() * 0.16,
                    u,
                ),
            )
        }
        Side::Top => Pos2::new(
            lerp(
                pad.left() + pad.width() * 0.18,
                pad.right() - pad.width() * 0.18,
                u,
            ),
            pad.top() + thick / 2.0,
        ),
    }
}

fn trail(p: &Painter, t: &Tokens, a: Pos2, b: Pos2, width: f32, alpha: f32) {
    const SEGS: usize = 8;
    for i in 0..SEGS {
        let (u0, u1) = (i as f32 / SEGS as f32, (i + 1) as f32 / SEGS as f32);
        p.line_segment(
            [a.lerp(b, u0), a.lerp(b, u1)],
            Stroke::new(width, fade(t.accent, alpha * (0.15 + 0.6 * u1))),
        );
    }
}

// ---------------------------------------------------------------------------------------------
// Step strip

/// What a step card shows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Mini {
    EdgePlace(Side),
    EdgeWait(Side),
    EdgeSlide(Side),
    RingPlace(usize),
    RingWait(usize),
    RingMove(usize),
    /// Fingers lifted, the assigned action runs.
    Run(usize),
    /// A glyph from the icon font on a rounded square.
    Icon(char),
}

fn mini(p: &Painter, t: &Tokens, card: Rect, kind: Mini, active: bool) {
    p.rect_filled(card, t.radius_lg as f32, t.input_bg);
    p.rect_stroke(
        card,
        t.radius_lg as f32,
        if active {
            Stroke::new(t.stroke_focus, t.accent)
        } else {
            Stroke::new(t.stroke_thin, t.border)
        },
        StrokeKind::Inside,
    );
    if let Mini::Icon(g) = kind {
        let r = Rect::from_center_size(card.center(), Vec2::splat(card.height() * 0.5));
        p.rect_filled(r, t.radius_md as f32, t.accent_soft);
        paint_icon(p, r.center(), g, card.height() * 0.3, t.accent);
        return;
    }
    let pad = card.shrink2(Vec2::new(card.width() * 0.1, card.height() * 0.14));
    pad_surface(p, t, pad);
    let dot = (pad.height() * 0.085).max(3.0);
    let thick = if matches!(
        kind,
        Mini::EdgePlace(Side::Top) | Mini::EdgeWait(Side::Top) | Mini::EdgeSlide(Side::Top)
    ) {
        pad.height() * 0.16
    } else {
        pad.width() * 0.12
    };
    let hand_at = |n: usize, shift: Vec2, alpha: f32| {
        for h in hand(n) {
            let c =
                pad.center() + Vec2::new(h.x * pad.width() * 0.8, h.y * pad.height() * 0.8) + shift;
            finger(p, t, c, dot, alpha);
        }
    };
    match kind {
        Mini::Icon(_) => {}
        Mini::EdgePlace(s) | Mini::EdgeWait(s) | Mini::EdgeSlide(s) => {
            paint_band(p, t, band_rect(pad, s, thick), true);
            let start = edge_point(pad, s, thick, 0.12);
            match kind {
                Mini::EdgePlace(_) => finger(p, t, start, dot, 1.0),
                Mini::EdgeWait(_) => {
                    finger(p, t, start, dot, 1.0);
                    dashed_ring(p, start, dot + 3.5, Stroke::new(1.3, t.accent));
                    clock_badge(p, t, card.right_top() + Vec2::new(-9.0, 9.0));
                }
                _ => {
                    let end = edge_point(pad, s, thick, 0.88);
                    finger(p, t, start, dot, 0.3);
                    trail(p, t, start, end, 3.0, 1.0);
                    finger(p, t, end, dot, 1.0);
                }
            }
        }
        Mini::RingPlace(n) => hand_at(n, Vec2::ZERO, 1.0),
        Mini::RingWait(n) => {
            hand_at(n, Vec2::ZERO, 1.0);
            for h in hand(n) {
                let c = pad.center() + Vec2::new(h.x * pad.width() * 0.8, h.y * pad.height() * 0.8);
                dashed_ring(p, c, dot + 3.0, Stroke::new(1.2, t.accent));
            }
            clock_badge(p, t, card.right_top() + Vec2::new(-9.0, 9.0));
        }
        Mini::RingMove(n) => {
            let shift = Vec2::new(pad.width() * 0.12, -pad.height() * 0.2);
            hand_at(n, Vec2::ZERO, 0.28);
            hand_at(n, shift, 1.0);
            arrow(
                p,
                pad.center() + Vec2::new(0.0, pad.height() * 0.1),
                pad.center() + shift + Vec2::new(0.0, pad.height() * 0.1),
                Stroke::new(1.5, t.accent),
            );
        }
        Mini::Run(n) => {
            for h in hand(n) {
                let c = pad.center() + Vec2::new(h.x * pad.width() * 0.8, h.y * pad.height() * 0.8);
                dashed_ring(p, c, dot, Stroke::new(1.0, fade(t.text_muted, 0.6)));
            }
            action_badge(p, t, pad.center(), pad.height() * 0.62);
        }
    }
}

/// A row of step cards with chevrons and a numbered caption under each (a column when narrow).
/// `active` marks the step the animation above is on.
pub fn steps(ui: &mut Ui, t: &Tokens, kinds: &[Mini], captions: &[&str], active: Option<usize>) {
    let n = captions.len().min(kinds.len());
    if n == 0 {
        return;
    }
    let lay = strip_layout(ui.available_width(), n);
    let card = Vec2::new(lay.card_w, (lay.card_w * 0.74).round());
    let font = FontId::proportional(t.fs_small);
    let galley = |ui: &Ui, i: usize, wrap: f32, active_i: bool| {
        let color = if active.is_none() || active_i {
            t.text
        } else {
            t.text_muted
        };
        ui.painter()
            .layout(captions[i].to_owned(), font.clone(), color, wrap)
    };
    let is_active = |i: usize| active == Some(i);
    if lay.vertical {
        let text_w = (ui.available_width() - card.x - 12.0 - 22.0).max(80.0);
        let galleys: Vec<_> = (0..n)
            .map(|i| galley(ui, i, text_w, is_active(i)))
            .collect();
        let row_h = |i: usize| card.y.max(galleys[i].size().y);
        let total: f32 = (0..n).map(row_h).sum::<f32>() + t.space_sm * (n - 1) as f32;
        let (rect, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), total), Sense::hover());
        let p = ui.painter_at(rect);
        let mut y = rect.top();
        for i in 0..n {
            let r = Rect::from_min_size(Pos2::new(rect.left(), y), card);
            mini(&p, t, r, kinds[i], is_active(i));
            let c = Pos2::new(r.right() + 12.0 + 7.5, y + 8.0);
            circled_number(&p, t, c, i + 1, is_active(i));
            p.galley(Pos2::new(c.x + 12.0, y), galleys[i].clone(), t.text);
            y += row_h(i) + t.space_sm;
        }
    } else {
        let wrap = card.x + CHEV_W - 8.0 - 20.0;
        let galleys: Vec<_> = (0..n).map(|i| galley(ui, i, wrap, is_active(i))).collect();
        let cap_h = galleys
            .iter()
            .map(|g| g.size().y)
            .fold(0.0, f32::max)
            .max(14.0);
        let total = Vec2::new(strip_width(n, card.x), card.y + t.space_sm + cap_h);
        let (rect, _) = ui.allocate_exact_size(total, Sense::hover());
        let p = ui.painter_at(rect.expand(2.0));
        for i in 0..n {
            let x = rect.left() + i as f32 * (card.x + CHEV_W);
            let r = Rect::from_min_size(Pos2::new(x, rect.top()), card);
            mini(&p, t, r, kinds[i], is_active(i));
            if i + 1 < n {
                chevron(&p, t, Pos2::new(r.right() + CHEV_W / 2.0, r.center().y));
            }
            let c = Pos2::new(x + 7.5, r.bottom() + t.space_sm + 8.0);
            circled_number(&p, t, c, i + 1, is_active(i));
            p.galley(
                Pos2::new(x + 20.0, r.bottom() + t.space_sm + 1.0),
                galleys[i].clone(),
                t.text,
            );
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Text beside an illustration

/// `left` (an illustration `left_w` wide) beside `right` when there is room, otherwise stacked.
pub fn two_col(
    ui: &mut Ui,
    t: &Tokens,
    left_w: f32,
    left: impl FnOnce(&mut Ui),
    right: impl FnOnce(&mut Ui),
) {
    let avail = ui.available_width();
    if side_by_side(avail, left_w) {
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width(left_w);
                left(ui);
            });
            ui.add_space(COL_GAP - ui.spacing().item_spacing.x);
            ui.vertical(|ui| {
                ui.set_width((avail - left_w - COL_GAP).max(MIN_TEXT_W));
                right(ui);
            });
        });
    } else {
        left(ui);
        ui.add_space(t.space_md);
        right(ui);
    }
}

/// Bold one-line summary and smaller secondary lines.
pub fn summary(ui: &mut Ui, t: &Tokens, title: &str, lines: &[String]) {
    ui.label(egui::RichText::new(title).font(t.font_semibold(t.fs_body)));
    ui.add_space(t.space_xs);
    for line in lines {
        ui.label(
            egui::RichText::new(line)
                .size(t.fs_small)
                .color(t.text_muted),
        );
        ui.add_space(3.0);
    }
}

// ---------------------------------------------------------------------------------------------
// Edge scene

/// Nominal Magic Trackpad size (mm); only the proportions matter.
const PAD_MM: (f32, f32) = (160.0, 115.0);
const SIDE_COLUMN_W: f32 = 78.0;

/// Width the edge scene takes, for `two_col`.
pub fn edge_scene_width(avail: f32) -> f32 {
    pad_width(avail) + SIDE_COLUMN_W
}

fn pad_width(avail: f32) -> f32 {
    (avail - SIDE_COLUMN_W).clamp(120.0, 270.0)
}

/// One edge of the pad as the scene draws it.
pub struct EdgeBand<'a> {
    pub side: Side,
    /// The target's short name; `None` when the edge does nothing.
    pub label: Option<&'a str>,
    /// Travel per level step (mm) for this edge's target.
    pub step_mm: f32,
}

/// The animated pad for the edge slider. Returns the strip step the animation is on.
pub fn edge_scene(
    ui: &mut Ui,
    t: &Tokens,
    anim: Anim,
    bands: &[EdgeBand<'_>],
    strip_mm: f32,
) -> usize {
    let w = pad_width(ui.available_width());
    let scale = w / PAD_MM.0;
    let pad_size = Vec2::new(w, PAD_MM.1 * scale);
    let (rect, _) =
        ui.allocate_exact_size(Vec2::new(w + SIDE_COLUMN_W, pad_size.y), Sense::hover());
    let p = ui.painter_at(rect.expand(2.0));
    let pad = Rect::from_min_size(rect.min, pad_size);
    pad_surface(&p, t, pad);
    let thick = (strip_mm * scale).clamp(4.0, w / 4.0);

    let shown: Vec<&EdgeBand<'_>> = bands.iter().filter(|b| b.label.is_some()).collect();
    // With no target on any edge there is nothing to demonstrate: a still pad and a note.
    let idle = shown.is_empty();
    let frame = edge_frame(
        if idle {
            EDGE_KEY_T
        } else {
            anim.at(EDGE_KEY_T)
        },
        shown.len().max(1),
    );
    let current = if idle {
        None
    } else {
        shown.get(frame.side).map(|b| b.side)
    };

    let dot = (w * 0.036).max(5.0);
    // Labels sit clear of the finger's path along the edge.
    let lab = dot * 2.0 + 8.0;
    let font = FontId::proportional(t.fs_small);
    for b in bands {
        let r = band_rect(pad, b.side, thick);
        if b.label.is_some() {
            paint_band(&p, t, r, Some(b.side) == current);
        }
        let text = format!("{}: {}", b.side.name(), b.label.unwrap_or("なし"));
        let color = if b.label.is_some() {
            t.text
        } else {
            t.text_muted
        };
        let (pos, anchor) = match b.side {
            Side::Left => (
                Pos2::new(pad.left() + lab, pad.center().y),
                Align2::LEFT_CENTER,
            ),
            Side::Right => (
                Pos2::new(pad.right() - lab, pad.center().y),
                Align2::RIGHT_CENTER,
            ),
            Side::Top => (
                Pos2::new(pad.center().x, pad.top() + lab),
                Align2::CENTER_TOP,
            ),
        };
        p.text(pos, anchor, text, font.clone(), color);
    }

    if idle {
        p.text(
            pad.center(),
            Align2::CENTER_CENTER,
            "端ごとの対象を選ぶと、
ここに動きが表示されます",
            FontId::proportional(t.fs_small),
            t.text_muted,
        );
    }
    let step_mm = shown.get(frame.side).map(|b| b.step_mm).filter(|_| !idle);

    if let Some(side) = current {
        let at = thick.max(dot * 2.0 + 2.0);
        let start = edge_point(pad, side, at, 0.0);
        let end = edge_point(pad, side, at, 1.0);
        // The path, with an arrow at the end.
        let guide = Stroke::new(1.0, fade(t.accent, 0.35));
        arrow(&p, start.lerp(end, 0.04), end, guide);
        let pos = start.lerp(end, frame.slide);
        trail(&p, t, start, pos, 3.5, frame.finger);
        finger(&p, t, pos, dot, frame.finger);
        if frame.dwell_alpha > 0.0 {
            timer_ring(&p, t, pos, dot + 5.0, frame.dwell, frame.dwell_alpha);
        }
    }

    // Level bar: segments filled one by one, as the detent distance is covered.
    let bar = Rect::from_min_size(
        Pos2::new(pad.right() + 16.0, pad.top() + 6.0),
        Vec2::new(10.0, pad.height() - 12.0),
    );
    let seg_h = (bar.height() - 3.0 * (EDGE_STEPS - 1) as f32) / EDGE_STEPS as f32;
    for i in 0..EDGE_STEPS {
        let y = bar.bottom() - (i + 1) as f32 * seg_h - i as f32 * 3.0;
        let r = Rect::from_min_size(Pos2::new(bar.left(), y), Vec2::new(bar.width(), seg_h));
        let on = !idle && i < frame.level;
        p.rect_filled(
            r,
            2.0,
            if on {
                fade(t.accent, 0.35 + 0.65 * frame.level_alpha)
            } else {
                track(t)
            },
        );
    }
    if let Some(mm) = step_mm {
        p.text(
            Pos2::new(bar.right() + 6.0, bar.center().y),
            Align2::LEFT_CENTER,
            format!("{mm:.1} mm\nごとに\n1 段階"),
            FontId::proportional(t.fs_small - 1.0),
            t.text_muted,
        );
        anim.keep_alive(ui, rect);
    }
    frame.step
}

// ---------------------------------------------------------------------------------------------
// Ring scene

const RING_PAD_FRAC: f32 = 0.40;

/// Width the ring scene takes.
pub fn ring_scene_width(avail: f32) -> f32 {
    avail.clamp(200.0, 440.0)
}

fn short_name(label: &str, index: usize) -> String {
    let label = label.trim();
    if label.is_empty() {
        return format!("項目 {}", index + 1);
    }
    let mut chars = label.chars();
    let head: String = chars.by_ref().take(7).collect();
    if chars.next().is_some() {
        format!("{}…", head.chars().take(6).collect::<String>())
    } else {
        head
    }
}

/// The animated pad and a mock of the ring. `labels` are the slots, clockwise from the top.
/// Returns the strip step the animation is on.
pub fn ring_scene(
    ui: &mut Ui,
    t: &Tokens,
    anim: Anim,
    fingers: usize,
    hold_ms: u16,
    labels: &[String],
) -> usize {
    let hold_s = f32::from(hold_ms) / 1000.0;
    let w = ring_scene_width(ui.available_width());
    let h = (w * 0.50).max(150.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, h), Sense::hover());
    let p = ui.painter_at(rect.expand(2.0));
    let frame = ring_frame(anim.at(ring_key_t(hold_s)), hold_s);

    // Pad on the left.
    let pad_w = w * RING_PAD_FRAC;
    let pad = Rect::from_center_size(
        Pos2::new(rect.left() + pad_w / 2.0, rect.center().y),
        Vec2::new(pad_w, pad_w * PAD_MM.1 / PAD_MM.0),
    );
    pad_surface(&p, t, pad);
    let slots = labels.len().clamp(1, 8);
    let slot = frame.cycle % slots;
    let angle_of =
        |i: usize| -std::f32::consts::FRAC_PI_2 + std::f32::consts::TAU * i as f32 / slots as f32;
    let dir = Vec2::angled(angle_of(slot));
    let shift = dir * pad.height() * 0.3 * frame.moved;
    let dot = (pad.height() * 0.085).max(4.0);
    let tips: Vec<Pos2> = hand(fingers)
        .iter()
        .map(|v| {
            pad.center() + Vec2::new(v.x * pad.width() * 0.8, v.y * pad.height() * 0.8) + shift
        })
        .collect();
    if frame.moved > 0.0 {
        let from = pad.center() + Vec2::new(0.0, pad.height() * 0.12);
        trail(&p, t, from, from + shift, 3.0, frame.fingers);
    }
    for c in &tips {
        finger(&p, t, *c, dot, frame.fingers);
    }
    if frame.hold_alpha > 0.0 {
        let extent = pad.height() * 0.2 + (fingers.clamp(1, 5) as f32 - 1.0) * pad.width() * 0.08;
        timer_ring(
            &p,
            t,
            pad.center() + Vec2::new(0.0, pad.height() * 0.06),
            extent,
            frame.hold,
            frame.hold_alpha,
        );
    }

    // "Screen" with the ring on the right.
    let panel = Rect::from_min_max(
        Pos2::new(pad.right() + w * 0.06, rect.top()),
        rect.right_bottom(),
    );
    p.rect_filled(panel, t.radius_lg as f32, t.input_bg);
    p.rect_stroke(
        panel,
        t.radius_lg as f32,
        Stroke::new(t.stroke_thin, t.border),
        StrokeKind::Inside,
    );
    let c = panel.center();
    p.circle_filled(c, 3.0, fade(t.accent, 0.4 + 0.6 * frame.open));
    if frame.open > 0.0 {
        let font = FontId::proportional(t.fs_small - 1.0);
        let (rx, ry) = (panel.width() / 2.0 - 46.0, panel.height() / 2.0 - 18.0);
        let a = frame.open;
        p.add(Shape::ellipse_stroke(
            c,
            Vec2::new(rx, ry) * frame.open,
            Stroke::new(1.0, fade(t.border, a)),
        ));
        for i in 0..slots {
            let ang = angle_of(i);
            let pos = c + Vec2::new(ang.cos() * rx, ang.sin() * ry) * frame.open;
            let label = short_name(labels.get(i).map_or("", String::as_str), i);
            let is_target = i == slot && frame.selected;
            let running = is_target && frame.flash > 0.0;
            let color = if running { t.on_accent } else { t.text };
            let g = p.layout_no_wrap(label, font.clone(), fade(color, a));
            let size = g.size() + Vec2::new(14.0, 8.0);
            let r = Rect::from_center_size(pos, size);
            let fill = if running {
                t.accent
            } else if is_target {
                t.accent_soft
            } else {
                t.control
            };
            p.rect_filled(r, size.y / 2.0, fade(fill, a));
            if is_target {
                p.rect_stroke(
                    r,
                    size.y / 2.0,
                    Stroke::new(t.stroke_thin, fade(t.accent, a)),
                    StrokeKind::Inside,
                );
            }
            p.galley(r.center() - g.size() / 2.0, g, fade(color, a));
            if running {
                let badge = p.layout_no_wrap(
                    "実行".to_owned(),
                    FontId::proportional(t.fs_small - 2.0),
                    t.on_accent,
                );
                let br = Rect::from_center_size(
                    r.right_top() + Vec2::new(-2.0, -2.0),
                    badge.size() + Vec2::new(8.0, 3.0),
                );
                let ba = (1.0 - frame.fade_out) * frame.flash;
                p.rect_filled(br, br.height() / 2.0, fade(t.accent, ba));
                p.rect_stroke(
                    br,
                    br.height() / 2.0,
                    Stroke::new(1.0, fade(t.card, ba)),
                    StrokeKind::Outside,
                );
                p.galley(
                    br.center() - badge.size() / 2.0,
                    badge,
                    fade(t.on_accent, ba),
                );
            }
        }
    }
    anim.keep_alive(ui, rect);
    frame.step
}

/// Draw the glyph used for the per-app page's cards (kept here so the page names no icon codes).
pub fn app_step_kinds() -> [Mini; 3] {
    [
        Mini::Icon(icons::APPS),
        Mini::Icon(icons::SETTINGS),
        Mini::Icon(icons::WINDOW),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ramp_and_ease_are_clamped() {
        assert_eq!(ramp(-1.0, 0.0, 1.0), 0.0);
        assert_eq!(ramp(2.0, 0.0, 1.0), 1.0);
        assert!((ramp(0.5, 0.0, 1.0) - 0.5).abs() < 1e-6);
        assert_eq!(ramp(0.0, 1.0, 1.0), 0.0);
        assert_eq!(ease(0.0), 0.0);
        assert_eq!(ease(1.0), 1.0);
        assert!((ease(0.5) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn edge_frame_follows_the_story() {
        let f = edge_frame(0.1, 1);
        assert_eq!((f.step, f.level), (0, 0));
        assert!(f.finger > 0.0 && f.finger < 1.0);
        let f = edge_frame(0.5, 1);
        assert_eq!(f.step, 1);
        assert!(f.dwell > 0.0 && f.dwell_alpha > 0.0);
        let f = edge_frame(EDGE_KEY_T, 1);
        assert_eq!(f.step, 2);
        assert!(f.slide > 0.3 && f.slide < 0.9 && f.level >= 2 && f.level < EDGE_STEPS);
        assert_eq!(f.finger, 1.0);
        let f = edge_frame(3.4, 1);
        assert_eq!(f.level, EDGE_STEPS);
        assert_eq!(edge_frame(3.95, 1).finger, 0.0);
    }

    #[test]
    fn edge_frame_alternates_sides_and_loops() {
        assert_eq!(edge_frame(1.0, 3).side, 0);
        assert_eq!(edge_frame(5.0, 3).side, 1);
        assert_eq!(edge_frame(9.0, 3).side, 2);
        assert_eq!(edge_frame(12.5, 3).side, 0);
        assert_eq!(edge_frame(1.0, 0).side, 0);
    }

    #[test]
    fn ring_frame_follows_the_story() {
        let hold = 0.6;
        let period = ring_period(hold);
        let f = ring_frame(0.1, hold);
        assert_eq!((f.step, f.cycle), (0, 0));
        assert_eq!(f.open, 0.0);
        let f = ring_frame(RING_LAND + 0.3, hold);
        assert_eq!(f.step, 1);
        assert!(f.hold > 0.0 && f.hold < 1.0 && f.hold_alpha > 0.0);
        let f = ring_frame(ring_key_t(hold), hold);
        assert_eq!(f.step, 2);
        assert!(f.open > 0.99 && f.selected && f.moved > 0.9);
        assert_eq!(f.flash, 0.0);
        let f = ring_frame(
            RING_LAND + hold + RING_MOVE_AT + RING_MOVE + RING_REST + RING_LIFT + 0.4,
            hold,
        );
        assert_eq!(f.step, 3);
        assert_eq!(f.fingers, 0.0);
        assert_eq!(f.flash, 1.0);
        assert_eq!(ring_frame(period + 0.1, hold).cycle, 1);
        assert_eq!(ring_frame(period - 0.05, hold).open, 0.0);
    }

    #[test]
    fn ring_period_grows_with_the_hold_time() {
        assert!(ring_period(1.5) > ring_period(0.3));
        assert!((ring_period(1.0) - ring_period(0.5) - 0.5).abs() < 1e-5);
    }

    #[test]
    fn strip_layout_breaks_to_a_column() {
        let wide = strip_layout(600.0, 4);
        assert!(!wide.vertical);
        assert_eq!(wide.card_w, CARD_W_MAX);
        let tight = strip_layout(400.0, 4);
        assert!(!tight.vertical && tight.card_w < CARD_W_MAX && tight.card_w >= CARD_W_MIN);
        assert!(strip_layout(300.0, 4).vertical);
        assert!(!strip_layout(300.0, 3).vertical);
        assert!(strip_layout(200.0, 3).vertical);
    }

    #[test]
    fn strip_width_counts_cards_and_chevrons() {
        assert_eq!(strip_width(3, 100.0), 300.0 + 2.0 * CHEV_W);
        assert_eq!(strip_width(1, 100.0), 100.0);
        assert_eq!(strip_width(0, 100.0), 0.0);
    }

    #[test]
    fn text_goes_beside_only_with_room() {
        assert!(side_by_side(800.0, 300.0));
        assert!(!side_by_side(500.0, 300.0));
        assert!(side_by_side(300.0 + COL_GAP + MIN_TEXT_W, 300.0));
    }

    #[test]
    fn hand_has_one_dot_per_finger() {
        for n in 1..=5 {
            assert_eq!(hand(n).len(), n);
        }
        assert_eq!(hand(9).len(), 5);
    }

    #[test]
    fn slot_names_are_shortened() {
        assert_eq!(short_name("コピー", 0), "コピー");
        assert_eq!(short_name("スクリーンショット", 3), "スクリーンシ…");
        assert_eq!(short_name("  ", 1), "項目 2");
    }
}
