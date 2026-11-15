//! 全般: theme, start with Windows, config folder and version.

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
        let before = cfg.general.start_with_windows;
        let st = crate::resident::autostart::status(before);
        let desc = if let Some(e) = &st.error {
            format!("自動起動を登録できませんでした: {e}")
        } else if st.not_registered {
            "自動起動を登録できませんでした（登録が見つかりません）。".to_string()
        } else if st.ram_disk_build {
            "開発用のビルドです。再起動で消えるため、自動起動には使えません".to_string()
        } else if st.target_missing {
            "登録された起動先が見つかりません。yubi を起動し直すと更新されます。".to_string()
        } else {
            "サインイン時にタスクトレイで開始します。".to_string()
        };
        g.toggle_row(
            None,
            "Windows の起動時に開始",
            &desc,
            &mut cfg.general.start_with_windows,
        );
        if cfg.general.start_with_windows != before
            && crate::resident::autostart::apply(cfg.general.start_with_windows).is_err()
        {
            // Keep the toggle honest: it stays as it was; the error shows on the next frame.
            cfg.general.start_with_windows = before;
        }
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
