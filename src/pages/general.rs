//! 全般: theme, start with Windows (placeholder), config folder and version.

use std::path::Path;

use eframe::egui::{RichText, Ui};

use super::Cmd;
use crate::config::{Config, ThemeChoice};
use crate::icons;
use crate::nav::Page;
use crate::theme::Tokens;
use crate::widgets::{self, button, group, section_header};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn general(ui: &mut Ui, t: &Tokens, cfg: &mut Config, config_path: &Path) -> Option<Cmd> {
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
