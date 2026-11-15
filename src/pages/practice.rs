//! 入力の確認: the live pad, what the recognizer decided, and a dry-run switch.

use std::time::{Duration, Instant};

use eframe::egui::{RichText, Ui};

use super::Cmd;
use super::diagnostics::{Diagnostics, empty_state};
use crate::engine::MonitorState;
use crate::engine::practice::{History, Judgement, Outcome, ago_label, idle_reason};
use crate::input::{Contact, DeviceInfo, TouchFrame};
use crate::nav::Page;
use crate::practice_gestures::Status;
use crate::theme::Tokens;
use crate::widgets::{self, group, section_header};

/// Same size as the old 入力モニター page; 接触 and 判定 sit below and scroll into view.
const PAD_MAX_HEIGHT: f32 = 360.0;

const REFRESH: Duration = Duration::from_millis(250);

impl Diagnostics {
    /// `practice_on` is the dry-run switch; the app turns it off again when the page is left.
    pub fn practice(
        &mut self,
        ui: &mut Ui,
        t: &Tokens,
        practice_on: &mut bool,
        win: Status<'_>,
    ) -> Option<Cmd> {
        widgets::page_header(ui, t, Page::Practice.label(), Page::Practice.subtitle());
        ui.ctx().request_repaint_after(REFRESH);

        group(ui, t, |g| {
            g.toggle_row(
                None,
                "練習モード",
                "オンの間は、認識したジェスチャーを表示するだけで実行しません。\
                 ほかのページへ移るか、ウィンドウを離れると自動でオフになります。",
                practice_on,
            );
        });
        let note = match win {
            Status::Quiet => None,
            Status::Active => Some((
                "練習中は Windows のタッチパッド ジェスチャーを一時的にオフにしています".to_owned(),
                t.text_muted,
            )),
            Status::Failed(e) => Some((
                format!(
                    "Windows のジェスチャーをオフにできませんでした。練習中は Windows 側の操作が重なって動くことがあります（{e}）"
                ),
                t.text_muted,
            )),
            Status::RestoreFailed(e) => Some((
                format!("Windows のジェスチャーを元に戻せませんでした（{e}）"),
                t.text,
            )),
        };
        let mut cmd = None;
        if let Some((text, color)) = note {
            group(ui, t, |g| {
                g.wrapped_row(|ui| {
                    ui.label(RichText::new(text).size(t.fs_small).color(color));
                });
                if matches!(win, Status::RestoreFailed(_)) {
                    g.wrapped_row(|ui| {
                        if widgets::button(ui, t, "もう一度元に戻す").clicked() {
                            cmd = Some(Cmd::RetryPracticeRestore);
                        }
                    });
                }
            });
        }

        let Some(device) = self.state.device.clone() else {
            empty_state(ui, t);
            return cmd;
        };
        ui.add_space(t.space_md);
        self.pad(ui, t, &device, PAD_MAX_HEIGHT);

        section_header(ui, t, "接触");
        let frame = self.state.frame.as_ref();
        let touching: Vec<&Contact> = frame
            .map(|f| f.contacts.iter().filter(|c| c.tip).collect())
            .unwrap_or_default();
        group(ui, t, |g| {
            g.info_row("指の本数", &self.state.finger_count.to_string());
            g.info_row("圧力", &pressure_text(&device, &touching));
        });

        section_header(ui, t, "判定");
        let s = &self.state;
        group(ui, t, |g| match s.practice.latest() {
            Some(j) => {
                g.info_row("ジェスチャー", &format!("{}（{}）", j.gesture, j.token));
                g.info_row("操作", j.action.as_deref().unwrap_or("—"));
                g.info_row("結果", j.outcome.note());
            }
            None => {
                g.wrapped_row(|ui| {
                    ui.label(
                        RichText::new(
                            "まだ認識されていません。トラックパッドで操作してみてください。",
                        )
                        .size(t.fs_small)
                        .color(t.text_muted),
                    );
                });
            }
        });
        if let Some(why) = idle_reason(
            s.gestures_enabled,
            s.paused,
            s.app_blocked,
            s.foreground_exe.as_deref(),
        ) {
            group(ui, t, |g| {
                g.wrapped_row(|ui| {
                    ui.label(RichText::new(why).size(t.fs_small));
                });
            });
        }

        section_header(ui, t, "最近の判定");
        group(ui, t, |g| {
            if s.practice.items().is_empty() {
                g.wrapped_row(|ui| {
                    ui.label(
                        RichText::new("履歴はまだありません。")
                            .size(t.fs_small)
                            .color(t.text_muted),
                    );
                });
            }
            for j in s.practice.items() {
                let title = match &j.action {
                    Some(a) => format!("{} → {a}", j.gesture),
                    None => j.gesture.clone(),
                };
                g.list_row(&title, &ago_label(j.at.elapsed()));
            }
        });
        cmd
    }

    /// Replace the live state with a fixed one (devshots only); `None` goes back to the engine.
    pub fn force(&mut self, state: Option<MonitorState>) {
        self.fake = state;
        if let Some(s) = &self.fake {
            self.state = s.clone();
        }
    }
}

/// "#0 0.42  #1 0.35", or why there is nothing to show.
fn pressure_text(device: &DeviceInfo, touching: &[&Contact]) -> String {
    if device.pressure_range.is_none() {
        return "非対応".to_owned();
    }
    let parts: Vec<String> = touching
        .iter()
        .filter_map(|c| c.pressure.map(|p| format!("#{} {p:.2}", c.id)))
        .collect();
    if parts.is_empty() {
        "—".to_owned()
    } else {
        parts.join("  ")
    }
}

/// A made-up state for screenshots: three fingers down and a few judgements.
pub fn fake_state() -> MonitorState {
    let device = std::sync::Arc::new(DeviceInfo {
        id: "devshot".to_owned(),
        name: "Magic Trackpad".to_owned(),
        x_range: (0, 12_000),
        y_range: (0, 8_600),
        pressure_range: Some((0, 255)),
        finger_slots: 5,
        size_mm: (160.0, 114.9),
    });
    let contact = |id: u32, x: f32, y: f32, pressure: f32| Contact {
        id,
        x,
        y,
        x_mm: x * 160.0,
        y_mm: y * 114.9,
        tip: true,
        confidence: true,
        pressure: Some(pressure),
    };
    let ago = |s: u64| {
        Instant::now()
            .checked_sub(Duration::from_secs(s))
            .unwrap_or_else(Instant::now)
    };
    let mut history = History::default();
    for (gesture, token, action, outcome, secs) in [
        (
            "4本指で右にスワイプ",
            "swipe.4.right",
            Some("次の仮想デスクトップ"),
            Outcome::DryRun,
            95,
        ),
        ("3本指タップ", "tap.3", None, Outcome::NoBinding, 40),
        (
            "3本指で上にスワイプ",
            "swipe.3.up",
            Some("タスクビュー"),
            Outcome::DryRun,
            4,
        ),
    ] {
        history.push(Judgement {
            gesture: gesture.to_owned(),
            token: token.to_owned(),
            action: action.map(str::to_owned),
            outcome,
            at: ago(secs),
        });
    }
    MonitorState {
        frame: Some(TouchFrame {
            contacts: vec![
                contact(0, 0.30, 0.55, 0.42),
                contact(1, 0.50, 0.50, 0.35),
                contact(2, 0.70, 0.56, 0.48),
            ],
            button: false,
            scan_time: 0,
            t_us: crate::input::now_us(),
            device: device.clone(),
        }),
        device: Some(device),
        finger_count: 3,
        gestures_enabled: true,
        practice: history,
        foreground_exe: Some("explorer.exe".to_owned()),
        ..MonitorState::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pressure_text_covers_each_case() {
        let mut state = fake_state();
        let device = state.device.take().unwrap();
        let contacts = state.frame.unwrap().contacts;
        let refs: Vec<&Contact> = contacts.iter().collect();
        assert_eq!(pressure_text(&device, &refs), "#0 0.42  #1 0.35  #2 0.48");
        assert_eq!(pressure_text(&device, &[]), "—");
        let mut flat = (*device).clone();
        flat.pressure_range = None;
        assert_eq!(pressure_text(&flat, &refs), "非対応");
    }
}
