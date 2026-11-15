//! ホーム: status header, quick access, saved assignments and the feature switchboard.

use eframe::egui::{Align, Layout, RichText, Sense, Ui, Vec2};

use super::Cmd;
use super::general::VERSION;
use crate::config::Config;
use crate::icons;
use crate::nav::{FEATURES, Page};
use crate::theme::Tokens;
use crate::widgets::{
    self, group, icon_button, paint_icon, section_header, tile, tile_width, toggle,
};

pub fn home(ui: &mut Ui, t: &Tokens, cfg: &mut Config, connected: bool) -> Option<Cmd> {
    let mut cmd = None;
    widgets::page_header(ui, t, Page::Home.label(), Page::Home.subtitle());
    header_row(ui, t, cfg, connected);
    ui.add_space(t.space_md);

    // Quick access spans the full width; below it the saved assignments and the gesture switches
    // sit side by side (their headers on one line), or stacked in a narrow window.
    cmd = quick_access(ui, t).or(cmd);
    if ui.available_width() >= t.two_col_min {
        ui.columns(2, |cols| {
            left_column(&mut cols[0], t, cfg);
            cmd = right_column(&mut cols[1], t, cfg).or(cmd);
        });
    } else {
        left_column(ui, t, cfg);
        cmd = right_column(ui, t, cfg).or(cmd);
    }
    cmd
}

fn header_row(ui: &mut Ui, t: &Tokens, cfg: &Config, connected: bool) {
    let (on, total) = cfg.enabled_count();
    let state = if connected {
        "接続済み"
    } else {
        "未接続"
    };
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(t.fs_icon_large), Sense::hover());
        paint_icon(
            ui.painter(),
            rect.center(),
            icons::TOUCHPAD,
            t.fs_icon_large,
            t.accent,
        );
        ui.label(RichText::new("yubi").font(t.font_semibold(t.fs_section)));
        ui.label(RichText::new(format!("バージョン {VERSION}")).color(t.text_muted));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(RichText::new(format!("有効 {on} / {total} 件")).color(t.text_muted));
            ui.label(RichText::new(format!("Magic Trackpad · {state}")).color(t.text_muted));
        });
    });
}

fn quick_access(ui: &mut Ui, t: &Tokens) -> Option<Cmd> {
    let mut cmd = None;
    section_header(ui, t, "クイックアクセス");
    group(ui, t, |g| {
        g.wrapped_row(|ui| {
            let labels = [
                Page::Practice.label(),
                "リングメニューを編集",
                "設定フォルダーを開く",
                "ログを書き出す",
            ];
            let w = tile_width(ui, t, &labels);
            {
                ui.spacing_mut().item_spacing = Vec2::splat(t.space_sm);
                if tile(ui, t, w, icons::MONITOR, labels[0], true).clicked() {
                    cmd = Some(Cmd::Goto(Page::Practice));
                }
                if tile(ui, t, w, icons::RING, labels[1], true).clicked() {
                    cmd = Some(Cmd::Goto(Page::Ring));
                }
                if tile(ui, t, w, icons::FOLDER, labels[2], true).clicked() {
                    cmd = Some(Cmd::OpenConfigFolder);
                }
                tile(ui, t, w, icons::LOG, labels[3], false).on_hover_text("未実装");
            }
        });
    });
    cmd
}

fn left_column(ui: &mut Ui, t: &Tokens, cfg: &Config) {
    section_header(ui, t, "保存された割り当て");
    group(ui, t, |g| {
        let list = cfg.assignments();
        if list.is_empty() {
            g.raw_row(|ui| {
                ui.label(RichText::new("有効な割り当てはありません。").color(t.text_muted));
            });
        }
        for (trigger, action) in &list {
            g.list_row(trigger, action);
        }
    });
}

fn right_column(ui: &mut Ui, t: &Tokens, cfg: &mut Config) -> Option<Cmd> {
    let mut cmd = None;
    section_header(ui, t, "ジェスチャー");
    let master = cfg.gestures_enabled;
    group(ui, t, |g| {
        g.toggle_row(
            Some(icons::TOUCHPAD),
            "ジェスチャー",
            "オフにすると、すべてのジェスチャーを無効にします。",
            &mut cfg.gestures_enabled,
        );
    });
    group(ui, t, |g| {
        for page in FEATURES {
            // The ジェスチャー page carries two switches (4 and 5 fingers): one row each.
            let rows: Vec<(char, &str, &str, bool)> = if page == Page::Gesture {
                vec![
                    (
                        icons::FOUR_FINGER,
                        "4本指ジェスチャー",
                        "4本指のタップ・スワイプ・ピンチに操作を割り当てます。",
                        true,
                    ),
                    (
                        icons::FIVE_FINGER,
                        "5本指ジェスチャー",
                        "5本指のタップ・スワイプ・ピンチに操作を割り当てます。",
                        false,
                    ),
                ]
            } else {
                vec![(page.icon(), page.label(), page.subtitle(), true)]
            };
            for (icon, label, desc, first) in rows {
                let mut on = if page == Page::Gesture && !first {
                    cfg.five.enabled
                } else {
                    page.is_on(cfg)
                };
                let desc = if desc.is_empty() {
                    page.subtitle()
                } else {
                    desc
                };
                g.row(Some(icon), label, desc, |ui| {
                    if icon_button(ui, t, icons::CHEVRON_RIGHT, "開く").clicked() {
                        cmd = Some(Cmd::Goto(page));
                    }
                    let was = on;
                    ui.add_enabled_ui(master, |ui| toggle(ui, t, &mut on));
                    if on != was {
                        if page == Page::Gesture && !first {
                            cfg.five.enabled = on;
                        } else {
                            page.set_on(cfg, on);
                        }
                    }
                });
            }
        }
    });
    cmd
}
