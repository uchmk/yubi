//! 診断: live view of the trackpad contacts and report statistics.

use std::sync::Arc;
use std::time::Duration;

use eframe::egui::{
    self, Align2, FontId, Frame, Margin, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Ui, Vec2,
};

use crate::input::{Contact, DeviceInfo, FpsCounter, InputEvent, TouchFrame};
use crate::theme::Tokens;

/// Tallest the pad drawing may get, so the stats stay visible in small windows.
const PAD_MAX_HEIGHT: f32 = 340.0;
const CONTACT_RADIUS: f32 = 14.0;
const CONTACT_RADIUS_PRESSURE_GAIN: f32 = 16.0;

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

    pub fn ui(&mut self, ui: &mut Ui, t: &Tokens, now: f64) {
        let Some(device) = self.device.clone() else {
            empty_state(ui, t);
            return;
        };
        ui.heading("診断");
        ui.add_space(t.space_md);
        self.pad(ui, t, &device);
        ui.add_space(t.space_lg);
        self.stats(ui, t, &device, now);
    }

    fn pad(&self, ui: &mut Ui, t: &Tokens, device: &DeviceInfo) {
        let aspect = device.aspect();
        let width = ui.available_width().min(PAD_MAX_HEIGHT * aspect).max(1.0);
        let (rect, _) = ui.allocate_exact_size(Vec2::new(width, width / aspect), Sense::hover());
        let pressed = self.frame.as_ref().is_some_and(|f| f.button);
        let painter = ui.painter_at(rect.expand(4.0));
        let radius = egui::CornerRadius::same(t.radius_lg);
        painter.rect_filled(rect, radius, t.surface_raised);
        let outline = if pressed {
            Stroke::new(3.0, t.accent)
        } else {
            Stroke::new(1.0, t.border)
        };
        painter.rect_stroke(rect, radius, outline, StrokeKind::Inside);

        let Some(frame) = &self.frame else { return };
        for c in &frame.contacts {
            draw_contact(&painter, t, rect, c);
        }
        if pressed {
            let galley = painter.layout_no_wrap(
                "クリック".to_owned(),
                FontId::proportional(13.0),
                t.on_accent,
            );
            let pos = rect.left_top() + Vec2::splat(t.space_md);
            let badge =
                Rect::from_min_size(pos, galley.size()).expand2(Vec2::new(t.space_sm, t.space_xs));
            painter.rect_filled(badge, egui::CornerRadius::same(t.radius_sm), t.accent);
            painter.galley(pos, galley, t.on_accent);
        }
    }

    fn stats(&mut self, ui: &mut Ui, t: &Tokens, device: &DeviceInfo, now: f64) {
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
        Frame::new()
            .fill(t.surface_raised)
            .stroke(Stroke::new(1.0, t.border))
            .corner_radius(t.radius_lg)
            .inner_margin(Margin::same(t.space_md as i8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                egui::Grid::new("diag-stats")
                    .num_columns(2)
                    .spacing(Vec2::new(t.space_lg, t.space_sm))
                    .show(ui, |ui| {
                        for (label, value) in rows {
                            ui.label(RichText::new(label).color(t.text_muted));
                            ui.add(egui::Label::new(RichText::new(value).monospace()).wrap());
                            ui.end_row();
                        }
                    });
            });
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
        painter.circle_filled(center, radius, color.gamma_multiply(0.55));
        painter.circle_stroke(center, radius, Stroke::new(1.5, color));
    } else {
        // Hovering contacts are hollow.
        painter.circle_stroke(center, radius, Stroke::new(2.0, color));
    }
    painter.text(
        center + Vec2::new(radius + 4.0, -radius),
        Align2::LEFT_BOTTOM,
        c.id.to_string(),
        FontId::monospace(12.0),
        t.text,
    );
}

fn empty_state(ui: &mut Ui, t: &Tokens) {
    ui.vertical_centered(|ui| {
        ui.add_space((ui.available_height() * 0.3).max(t.space_lg));
        ui.label(RichText::new("トラックパッドが見つかりません").size(18.0));
        ui.add_space(t.space_sm);
        ui.label(
            RichText::new("USB か Bluetooth で接続し、ドライバを入れてください。")
                .color(t.text_muted),
        );
    });
}
