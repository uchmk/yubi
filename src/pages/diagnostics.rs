//! 診断 (device info, ranges, counters).

use std::time::Duration;

use eframe::egui::{
    self, Align2, FontId, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Ui, Vec2,
};

use crate::engine::MonitorState;
use crate::engine::dispatch::{corner_swipe_label, multi_label, press_label};
use crate::engine::recognizer::GestureEvent;
use crate::input::{Contact, DeviceInfo, now_us};
use crate::nav::Page;
use crate::theme::Tokens;
use crate::widgets::{self, group, section_header};

/// Tallest the pad drawing may get, so the page stays usable in small windows.
const CONTACT_RADIUS: f32 = 14.0;
const CONTACT_RADIUS_PRESSURE_GAIN: f32 = 16.0;
const CONTACT_STROKE: f32 = 1.5;
const HOVER_STROKE: f32 = 2.0;
const PRESSED_STROKE: f32 = 3.0;
const CONTACT_FILL_ALPHA: f32 = 0.55;

/// Frames older than this no longer count towards the live rate.
const STALE_US: u64 = 1_000_000;

#[derive(Default)]
pub struct Diagnostics {
    /// Copy of the engine's latest state, refreshed once per UI frame.
    pub(super) state: MonitorState,
    /// A fixed state for screenshots; the engine's is ignored while it is set.
    pub(super) fake: Option<MonitorState>,
}

impl Diagnostics {
    pub fn update(&mut self, state: MonitorState) {
        if self.fake.is_none() {
            self.state = state;
        }
    }

    /// The engine's latest state (for the 押し込みと触覚 meter).
    pub fn state(&self) -> &MonitorState {
        &self.state
    }

    /// True once a trackpad was seen and not removed since.
    pub fn connected(&self) -> bool {
        self.state.device.is_some()
    }

    /// Frames per second, 0 once the stream has stopped.
    fn rate(&self) -> usize {
        if now_us().saturating_sub(self.state.last_frame_us) > STALE_US {
            0
        } else {
            self.state.fps
        }
    }

    pub fn info(&mut self, ui: &mut Ui, t: &Tokens, _now: f64) {
        widgets::page_header(
            ui,
            t,
            Page::Diagnostics.label(),
            Page::Diagnostics.subtitle(),
        );
        let rate = self.rate();
        if rate > 0 {
            ui.ctx().request_repaint_after(Duration::from_millis(250));
        }
        if let Some(device) = self.state.device.clone() {
            let frame = self.state.frame.as_ref();
            let contacts = frame.map_or(0, |f| f.contacts.len());
            let pressures = frame
                .map(|f| {
                    f.contacts
                        .iter()
                        .filter_map(|c| c.pressure.map(|p| format!("#{} {p:.2}", c.id)))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let pressure_text = if device.pressure_range.is_none() {
                "非対応".to_owned()
            } else if pressures.is_empty() {
                "—".to_owned()
            } else {
                pressures.join("  ")
            };
            let range = |r: (i32, i32)| format!("{} … {}", r.0, r.1);
            let rows: Vec<(&str, String)> = vec![
                ("デバイス", device.name.clone()),
                ("デバイス ID", device.id.clone()),
                (
                    "接触数",
                    format!("{contacts}（最大 {}）", self.state.peak_contacts),
                ),
                (
                    "スキャン時間",
                    frame.map_or("—".into(), |f| f.scan_time.to_string()),
                ),
                ("レポートレート", format!("{rate} フレーム/秒")),
                ("圧力", pressure_text),
                ("X の範囲", range(device.x_range)),
                ("Y の範囲", range(device.y_range)),
                (
                    "圧力の範囲",
                    device.pressure_range.map_or("—".into(), range),
                ),
                (
                    "物理サイズ",
                    format!("{:.1} × {:.1} mm", device.size_mm.0, device.size_mm.1),
                ),
                ("指スロット数", device.finger_slots.to_string()),
            ];
            group(ui, t, |g| {
                for (label, value) in &rows {
                    g.info_row(label, value);
                }
            });
        } else {
            empty_state(ui, t);
        }

        section_header(ui, t, "入力とエンジン");
        let s = &self.state;
        let mut rows: Vec<(&str, String)> = vec![
            (
                "エンジン",
                if s.gestures_enabled {
                    "動作中".to_owned()
                } else {
                    "停止中（ジェスチャーが無効）".to_owned()
                },
            ),
            ("認識中の指の本数", s.finger_count.to_string()),
            (
                "最後のジェスチャー",
                s.last_event.map_or("—".into(), event_label),
            ),
            (
                "最後の操作",
                s.last_action.clone().unwrap_or_else(|| "—".into()),
            ),
            (
                "前面のアプリ",
                s.foreground_exe.clone().unwrap_or_else(|| "—".into()),
            ),
            (
                "このアプリでの yubi",
                if s.app_blocked {
                    "無効（アプリ設定で除外）".to_owned()
                } else {
                    "有効".to_owned()
                },
            ),
            ("受信フレーム合計", s.frames_total.to_string()),
            ("破棄した部分フレーム", s.partial_frames.to_string()),
            ("破棄した接触", s.dropped_contacts.to_string()),
            (
                "前回のフレーム間隔",
                s.last_delta_ms
                    .map_or("—".into(), |ms| format!("{ms:.1} ms")),
            ),
            (
                "非フォーカス中に受信したフレーム",
                s.frames_background.to_string(),
            ),
        ];
        if let Some(e) = &s.failed {
            rows.push(("入力スレッドのエラー", e.clone()));
        }
        if let Some(e) = &s.session_error {
            rows.push(("セッションの監視", e.clone()));
        }
        if let Some(e) = &s.record_error {
            rows.push(("記録のエラー", e.clone()));
        }
        group(ui, t, |g| {
            for (label, value) in &rows {
                g.info_row(label, value);
            }
        });
    }

    pub(super) fn pad(&self, ui: &mut Ui, t: &Tokens, device: &DeviceInfo, max_height: f32) {
        let aspect = device.aspect();
        let width = ui.available_width().min(max_height * aspect).max(1.0);
        let (rect, _) = ui.allocate_exact_size(Vec2::new(width, width / aspect), Sense::hover());
        let pressed = self.state.frame.as_ref().is_some_and(|f| f.button);
        let painter = ui.painter_at(rect.expand(t.space_xs));
        let radius = egui::CornerRadius::same(t.radius_lg);
        painter.rect_filled(rect, radius, t.card);
        let outline = if pressed {
            Stroke::new(PRESSED_STROKE, t.accent)
        } else {
            Stroke::new(t.stroke_thin, t.border)
        };
        painter.rect_stroke(rect, radius, outline, StrokeKind::Inside);

        let touching = self
            .state
            .frame
            .as_ref()
            .is_some_and(|f| any_tip(&f.contacts));
        if !touching {
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                "トラックパッドに指を置くと、ここに表示されます。",
                t.font(t.fs_body),
                t.text_muted,
            );
        }
        // After the last finger lifts the final frame still holds hovering (hollow) contacts;
        // show the hint instead of them. Hollow circles appear only while a finger is down.
        let Some(frame) = self.state.frame.as_ref().filter(|_| touching) else {
            return;
        };
        for c in &frame.contacts {
            draw_contact(&painter, t, rect, c);
        }
        if pressed {
            let galley =
                painter.layout_no_wrap("クリック".to_owned(), t.font(t.fs_small), t.on_accent);
            let pos = rect.left_top() + Vec2::splat(t.space_md);
            let badge =
                Rect::from_min_size(pos, galley.size()).expand2(Vec2::new(t.space_sm, t.space_xs));
            painter.rect_filled(badge, egui::CornerRadius::same(t.radius_sm), t.accent);
            painter.galley(pos, galley, t.on_accent);
        }
    }
}

/// True if at least one contact is touching the pad (tip switch on).
fn any_tip(contacts: &[Contact]) -> bool {
    contacts.iter().any(|c| c.tip)
}

fn draw_contact(painter: &egui::Painter, t: &Tokens, pad: Rect, c: &Contact) {
    let center = Pos2::new(
        pad.left() + c.x * pad.width(),
        pad.top() + c.y * pad.height(),
    );
    let radius = CONTACT_RADIUS + c.pressure.unwrap_or(0.0) * CONTACT_RADIUS_PRESSURE_GAIN;
    let color = if c.confidence { t.accent } else { t.text_muted };
    if c.tip {
        painter.circle_filled(center, radius, color.gamma_multiply(CONTACT_FILL_ALPHA));
        painter.circle_stroke(center, radius, Stroke::new(CONTACT_STROKE, color));
    } else {
        // Hovering contacts are hollow.
        painter.circle_stroke(center, radius, Stroke::new(HOVER_STROKE, color));
    }
    painter.text(
        center + Vec2::new(radius + t.space_xs, -radius),
        Align2::LEFT_BOTTOM,
        c.id.to_string(),
        FontId::monospace(t.fs_small),
        t.text,
    );
}

/// Short Japanese description of a recognizer event.
fn event_label(e: GestureEvent) -> String {
    match e {
        GestureEvent::FingerCount(n) => format!("指の本数 {n}"),
        GestureEvent::TouchStart { fingers } => format!("タッチ開始（{fingers} 本）"),
        GestureEvent::TouchEnd {
            fingers,
            duration_us,
        } => format!("タッチ終了（{fingers} 本、{} ms）", duration_us / 1000),
        GestureEvent::Tap { fingers } => format!("タップ（{fingers} 本）"),
        GestureEvent::LongHold { fingers } => format!("長押し（{fingers} 本）"),
        GestureEvent::Multi { fingers, kind } => multi_label(fingers, kind),
        GestureEvent::Press { fingers, deep } => press_label(fingers, deep),
        GestureEvent::Corner { corner, slot } => corner_swipe_label(corner, slot),
    }
}

pub(super) fn empty_state(ui: &mut Ui, t: &Tokens) {
    ui.add_space(t.space_xl);
    ui.vertical_centered(|ui| {
        ui.label(RichText::new("トラックパッドが見つかりません").size(t.fs_section));
        ui.add_space(t.space_sm);
        ui.label(
            RichText::new("USB か Bluetooth で接続し、ドライバを入れてください。")
                .color(t.text_muted),
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contact(tip: bool) -> Contact {
        Contact {
            id: 0,
            x: 0.5,
            y: 0.5,
            x_mm: 0.0,
            y_mm: 0.0,
            tip,
            confidence: true,
            pressure: None,
        }
    }

    #[test]
    fn hover_only_frame_counts_as_empty_pad() {
        assert!(!any_tip(&[]));
        assert!(!any_tip(&[contact(false), contact(false)]));
        assert!(any_tip(&[contact(false), contact(true)]));
    }
}
