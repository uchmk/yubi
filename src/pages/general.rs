//! 全般: theme, start with Windows, config folder and version.

use std::path::Path;

use eframe::egui::{RichText, Ui};

use super::Cmd;
use super::general_io::{GeneralState, HapticStatus, Note, Preview};
use crate::config::{Config, ThemeChoice};
use crate::icons;
use crate::nav::Page;
use crate::theme::Tokens;
use crate::widgets::{self, accent_button, button, group, section_header};
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
    st: &mut GeneralState,
) -> Option<Cmd> {
    let mut cmd = None;
    st.poll(cfg);
    st.demo(cfg);
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
    section_header(ui, t, "設定の書き出しと読み込み");
    group(ui, t, |g| transfer_rows(g, t, cfg, config_path, st));
    section_header(ui, t, "触覚フィードバックの許可");
    group(ui, t, |g| haptic_rows(g, t, st));
    section_header(ui, t, "バージョン情報");
    group(ui, t, |g| {
        g.row(None, "yubi", "", |ui| {
            ui.label(RichText::new(VERSION).color(t.text_muted));
        });
    });
    cmd
}

fn note_row(g: &mut widgets::Group<'_>, t: &Tokens, note: &Note) {
    g.wrapped_row(|ui| {
        let color = if note.is_error { t.text } else { t.text_muted };
        ui.label(RichText::new(&note.text).size(t.fs_small).color(color));
    });
}

fn transfer_rows(
    g: &mut widgets::Group<'_>,
    t: &Tokens,
    cfg: &mut Config,
    config_path: &Path,
    st: &mut GeneralState,
) {
    g.wrapped_row(|ui| {
        ui.label(
            RichText::new(
                "設定を TOML ファイルに保存したり、別の PC の設定を読み込んだりします。読み込む前に変更内容を確認でき、元に戻すこともできます。",
            )
            .size(t.fs_small)
            .color(t.text_muted),
        );
    });
    g.wrapped_row(|ui| {
        let idle = !st.io_busy && st.preview.is_none();
        ui.add_enabled_ui(idle, |ui| {
            if button(ui, t, "設定を書き出す…").clicked() {
                st.start_export(cfg);
            }
            if button(ui, t, "設定を読み込む…").clicked() {
                st.start_import();
            }
        });
        ui.add_enabled_ui(st.can_undo() && !st.io_busy, |ui| {
            if button(ui, t, "元に戻す")
                .on_hover_text(
                    "読み込み前の状態にすべての設定を戻します。その後の変更は失われます。",
                )
                .clicked()
            {
                st.undo_import(cfg, config_path);
            }
        });
        if st.can_undo() && !st.io_busy {
            ui.label(
                RichText::new(
                    "元に戻すと、読み込み前の状態にすべての設定が戻り、その後の変更は失われます。",
                )
                .size(t.fs_small)
                .color(t.text_muted),
            );
        }
        if st.io_busy {
            ui.label(
                RichText::new("ファイルの選択を待っています…")
                    .size(t.fs_small)
                    .color(t.text_muted),
            );
        }
    });
    if let Some(note) = st.io_note.clone() {
        note_row(g, t, &note);
    }
    if let Some(p) = st.preview.clone() {
        preview_rows(g, t, &p);
        g.wrapped_row(|ui| {
            let changed = !p.changes.is_empty();
            ui.add_enabled_ui(changed, |ui| {
                if accent_button(ui, t, "適用").clicked() {
                    st.apply_preview(cfg, config_path);
                }
            });
            if button(ui, t, "キャンセル").clicked() {
                st.cancel_preview();
            }
        });
    }
}

fn preview_rows(g: &mut widgets::Group<'_>, t: &Tokens, p: &Preview) {
    let file = p
        .path
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    g.info_row("読み込むファイル", &file);
    if p.changes.is_empty() {
        g.wrapped_row(|ui| {
            ui.label(RichText::new("現在の設定と同じです。変更はありません。").size(t.fs_body));
        });
    }
    for c in &p.changes {
        g.wrapped_row(|ui| {
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                ui.label(
                    RichText::new(format!("{} — {} 項目が変わります", c.section, c.count))
                        .size(t.fs_body),
                );
                for d in &c.details {
                    ui.label(RichText::new(d).size(t.fs_small).color(t.text_muted));
                }
            });
        });
    }
    for w in &p.imported.warnings {
        g.wrapped_row(|ui| {
            ui.label(RichText::new(format!("補正: {w}")).size(t.fs_small));
        });
    }
    g.wrapped_row(|ui| {
        ui.label(
            RichText::new("「Windows の起動時に開始」は読み込まれません。")
                .size(t.fs_small)
                .color(t.text_muted),
        );
    });
}

fn haptic_rows(g: &mut widgets::Group<'_>, t: &Tokens, st: &mut GeneralState) {
    let (status, desc) = match st.haptic {
        HapticStatus::Checking => ("確認中…", "ドライバーの設定を確認しています。"),
        HapticStatus::Granted => ("許可済み", "yubi は触覚フィードバックを使えます。"),
        HapticStatus::NotGranted => (
            "未許可",
            "ドライバーの設定を管理者権限で一度だけ変更します。Windows の確認画面が表示されます。",
        ),
        HapticStatus::Unknown => (
            "確認できません",
            "ドライバーの設定を読み取れませんでした。ドライバーがインストールされているか確認してください。",
        ),
    };
    let grant = status_row(g, t, st.haptic, status, desc);
    if grant {
        st.start_haptic_grant();
    }
    if let Some(note) = st.haptic_note.clone() {
        note_row(g, t, &note);
    }
}

/// The status row; true when 許可する… was pressed.
fn status_row(
    g: &mut widgets::Group<'_>,
    t: &Tokens,
    status: HapticStatus,
    label: &str,
    desc: &str,
) -> bool {
    let mut pressed = false;
    g.row(None, "触覚フィードバック", desc, |ui| {
        if status == HapticStatus::NotGranted && button(ui, t, "許可する…").clicked() {
            pressed = true;
        }
        ui.label(RichText::new(label).color(t.text_muted));
    });
    pressed
}
