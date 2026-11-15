//! 入力モニター (live contact view) and 診断 (device info, ranges, counters).

use std::sync::Arc;
use std::time::Duration;

use eframe::egui::{
    self, Align2, FontId, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Ui, Vec2,
};

use crate::input::{Contact, DeviceInfo, FpsCounter, InputEvent, TouchFrame};
use crate::nav::Page;
use crate::theme::Tokens;
use crate::widgets::{self, group};

/// Tallest the pad drawing may get, so the page stays usable in small windows.
const PAD_MAX_HEIGHT: f32 = 360.0;
const CONTACT_RADIUS: f32 = 14.0;
const CONTACT_RADIUS_PRESSURE_GAIN: f32 = 16.0;
const CONTACT_STROKE: f32 = 1.5;
const HOVER_STROKE: f32 = 2.0;
const PRESSED_STROKE: f32 = 3.0;
const CONTACT_FILL_ALPHA: f32 = 0.55;

#[derive(Default)]
pub struct Diagnostics {
    device: Option<Arc<DeviceInfo>>,
    frame: Option<TouchFrame>,
    fps: FpsCounter,
    peak_contacts: usize,
    frames_total: u64,
}

impl Diagnostics {
    pub fn ingest(&mut self, ev: InputEvent, now: f64) {
        match ev {
            InputEvent::Frame(f) => {
                self.fps.record(now);
                self.frames_total += 1;
                self.peak_contacts = self.peak_contacts.max(f.contacts.len());
                self.device = Some(f.device.clone());
                self.frame = Some(f);
            }
            InputEvent::DeviceArrived(d) => self.device = Some(d),
            InputEvent::DeviceRemoved(name) => {
                if self.device.as_ref().is_some_and(|d| d.name == name) {
                    self.device = None;
                    self.frame = None;
                }
            }
        }
    }

    /// True once a trackpad was seen and not removed since.
    pub fn connected(&self) -> bool {
        self.device.is_some()
    }

    pub fn monitor(&mut self, ui: &mut Ui, t: &Tokens) {
        widgets::page_header(ui, t, Page::Monitor.label(), Page::Monitor.subtitle());
        let Some(device) = self.device.clone() else {
            empty_state(ui, t);
            return;
        };
        self.pad(ui, t, &device);
        let rate = self.fps.rate(ui.input(|i| i.time));
        if rate > 0 {
            ui.ctx().request_repaint_after(Duration::from_millis(250));
        }
    }

    pub fn info(&mut self, ui: &mut Ui, t: &Tokens, now: f64) {
        widgets::page_header(
            ui,
            t,
            Page::Diagnostics.label(),
            Page::Diagnostics.subtitle(),
        );
        let Some(device) = self.device.clone() else {
            empty_state(ui, t);
            return;
        };
        let rate = self.fps.rate(now);
        if rate > 0 {
            ui.ctx().request_repaint_after(Duration::from_millis(250));
        }
        let frame = self.frame.as_ref();
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
            (
                "接触数",
                format!("{contacts}（最大 {}）", self.peak_contacts),
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
            ("指スロット数", device.finger_slots.to_string()),
            ("受信フレーム合計", self.frames_total.to_string()),
        ];
        group(ui, t, |g| {
            for (label, value) in &rows {
                g.info_row(label, value);
            }
        });
    }

    fn pad(&self, ui: &mut Ui, t: &Tokens, device: &DeviceInfo) {
        let aspect = device.aspect();
        let width = ui.available_width().min(PAD_MAX_HEIGHT * aspect).max(1.0);
        let (rect, _) = ui.allocate_exact_size(Vec2::new(width, width / aspect), Sense::hover());
        let pressed = self.frame.as_ref().is_some_and(|f| f.button);
        let painter = ui.painter_at(rect.expand(t.space_xs));
        let radius = egui::CornerRadius::same(t.radius_lg);
        painter.rect_filled(rect, radius, t.card);
        let outline = if pressed {
            Stroke::new(PRESSED_STROKE, t.accent)
        } else {
            Stroke::new(t.stroke_thin, t.border)
        };
        painter.rect_stroke(rect, radius, outline, StrokeKind::Inside);

        if self.frame.as_ref().is_none_or(|f| f.contacts.is_empty()) {
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                "トラックパッドに指を置くと、ここに表示されます。",
                t.font(t.fs_body),
                t.text_muted,
            );
        }
        let Some(frame) = &self.frame else { return };
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

fn empty_state(ui: &mut Ui, t: &Tokens) {
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
