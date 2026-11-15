//! 全般: theme, start with Windows (placeholder), config folder and version.

use std::path::Path;

use eframe::egui::{RichText, Ui};

use super::Cmd;
use crate::config::{Config, ThemeChoice};
use crate::icons;
use crate::nav::Page;
use crate::theme::Tokens;
use crate::widgets::{self, button, group, section_header};
use crate::win_gestures::{self, LABELS, Values};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// What the 全般 page shows about Windows' own gestures.
pub struct WinGestureView<'a> {
    pub values: &'a Values,
    pub backup_exists: bool,
    /// Result of the last button press.
    pub message: Option<&'a str>,
}

pub fn general(
    ui: &mut Ui,
    t: &Tokens,
    cfg: &mut Config,
    config_path: &Path,
    win: &WinGestureView<'_>,
) -> Option<Cmd> {
    let mut cmd = None;
    widgets::page_header(ui, t, Page::General.label(), Page::General.subtitle());
    section_header(ui, t, "外観");
    group(ui, t, |g| {
        g.combo_row(
            "テーマ",
            "既定は黒です。「システムに合わせる」は Windows の設定に従います。",
            "theme",
            &mut cfg.general.theme,
            &ThemeChoice::ALL,
            ThemeChoice::label,
        );
    });
    section_header(ui, t, "起動");
    group(ui, t, |g| {
        g.toggle_row(
            None,
            "Windows の起動時に開始",
            "この設定はまだ反映されません。",
            &mut cfg.general.start_with_windows,
        );
    });
    section_header(ui, t, "Windows のジェスチャーとの競合");
    group(ui, t, |g| {
        for (i, label) in LABELS.iter().enumerate() {
            g.list_row(label, win_gestures::describe(win.values[i]));
        }
        g.wrapped_row(|ui| {
            ui.label(
                RichText::new(
                    "yubi の 3・4 本指の機能は、Windows 標準の同じジェスチャーと重なります。\
                     変更はサインアウトしないと反映されないことがあります。",
                )
                .size(t.fs_small)
                .color(t.text_muted),
            );
        });
        g.wrapped_row(|ui| {
            let can_disable = win_gestures::any_enabled(win.values);
            ui.add_enabled_ui(can_disable, |ui| {
                if button(ui, t, "3・4本指のジェスチャーを Windows で無効にする").clicked()
                {
                    cmd = Some(Cmd::DisableWindowsGestures);
                }
            });
            ui.add_enabled_ui(win.backup_exists, |ui| {
                if button(ui, t, "元に戻す").clicked() {
                    cmd = Some(Cmd::RestoreWindowsGestures);
                }
            });
        });
        if let Some(m) = win.message {
            g.wrapped_row(|ui| {
                ui.label(RichText::new(m).size(t.fs_small));
            });
        }
    });
    section_header(ui, t, "設定ファイル");
    group(ui, t, |g| {
        let folder = config_path
            .parent()
            .map_or_else(String::new, |p| p.display().to_string());
        g.row(Some(icons::FOLDER), "設定フォルダー", &folder, |ui| {
            if button(ui, t, "開く").clicked() {
                cmd = Some(Cmd::OpenConfigFolder);
            }
        });
    });
    section_header(ui, t, "バージョン情報");
    group(ui, t, |g| {
        g.row(None, "yubi", "", |ui| {
            ui.label(RichText::new(VERSION).color(t.text_muted));
        });
    });
    cmd
}
