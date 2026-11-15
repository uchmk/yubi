//! 全般: theme, start with Windows, config folder and version.

use std::path::Path;

use eframe::egui::{self, Id, Key, Modifiers, RichText, Ui};

use super::Cmd;
use super::general_io::{GeneralState, HapticStatus, Note, Preview, READ_ONLY_REASON};
use crate::config::{Config, ThemeChoice};
use crate::engine::pause::{PauseView, remaining_label};
use crate::icons;
use crate::nav::Page;
use crate::resident::ShortcutStatus;
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

/// What the 一時停止 section shows.
#[derive(Clone, Copy)]
pub struct PauseInfo {
    pub shortcut: ShortcutStatus,
    pub pause: PauseView,
}

pub fn general(
    ui: &mut Ui,
    t: &Tokens,
    cfg: &mut Config,
    config_path: &Path,
    win: &WinGestureView<'_>,
    st: &mut GeneralState,
    pause: PauseInfo,
) -> Option<Cmd> {
    let mut cmd = None;
    st.poll(cfg);
    st.demo(cfg);
    widgets::page_header(ui, t, Page::General.label(), Page::General.subtitle());
    widgets::sections2(ui, t, 5, 9, |ui, i| match i {
        0 => {
            if !st.load_notes.is_empty() {
                section_header(ui, t, "起動時の確認");
                let notes = st.load_notes.clone();
                let mut dismiss = false;
                group(ui, t, |g| {
                    for n in &notes {
                        g.wrapped_row(|ui| {
                            ui.label(RichText::new(n).size(t.fs_small).color(t.text));
                        });
                    }
                    g.wrapped_row(|ui| {
                        dismiss = button(ui, t, "閉じる").clicked();
                    });
                });
                if dismiss {
                    st.load_notes.clear();
                }
            }
        }
        1 => {
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
        }
        2 => {
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
                    "登録された起動先が見つかりません。yubi を起動し直すと更新されます。"
                        .to_string()
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
        }
        3 => {
            section_header(ui, t, "一時停止");
            let ctx = ui.ctx().clone();
            group(ui, t, |g| pause_rows(g, t, &ctx, cfg, pause, &mut cmd));
        }
        4 => {
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
        }
        5 => {
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
        }
        6 => {
            section_header(ui, t, "設定の書き出しと読み込み");
            group(ui, t, |g| transfer_rows(g, t, cfg, config_path, st));
        }
        7 => {
            section_header(ui, t, "触覚フィードバックの許可");
            group(ui, t, |g| haptic_rows(g, t, st));
        }
        8 => {
            section_header(ui, t, "バージョン情報");
            group(ui, t, |g| {
                g.row(None, "yubi", "", |ui| {
                    ui.label(RichText::new(VERSION).color(t.text_muted));
                });
            });
        }
        _ => {}
    });
    cmd
}

/// Egui temp-data flag: the shortcut field is waiting for a key press.
fn capture_id() -> Id {
    Id::unique("pause-shortcut-capture")
}

fn pause_rows(
    g: &mut widgets::Group<'_>,
    t: &Tokens,
    ctx: &egui::Context,
    cfg: &mut Config,
    info: PauseInfo,
    cmd: &mut Option<Cmd>,
) {
    let PauseInfo { shortcut, pause } = info;
    g.row(None, "yubi の状態", &pause_state_text(pause), |ui| {
        let label = if pause.paused {
            "再開"
        } else {
            "一時停止"
        };
        if button(ui, t, label).clicked() {
            *cmd = Some(Cmd::TogglePause);
        }
    });
    let id = capture_id();
    let mut capturing = ctx.data(|d| d.get_temp::<bool>(id)).unwrap_or(false);
    let mut hint = None;
    if capturing {
        match capture_result(ctx) {
            Capture::Waiting => {}
            Capture::NeedsModifier => hint = Some("Ctrl か Alt と一緒に押してください"),
            Capture::Cancel => capturing = false,
            Capture::Got(text) => {
                cfg.general.pause_shortcut = text;
                capturing = false;
            }
        }
        ctx.request_repaint();
    }
    let current = if cfg.general.pause_shortcut.is_empty() {
        "未設定".to_owned()
    } else {
        cfg.general.pause_shortcut.clone()
    };
    g.row(
        None,
        "一時停止のショートカット",
        "どのアプリの上でも、yubi の一時停止と再開を切り替えます。トレイのアイコンからも操作できます。",
        |ui| {
            if capturing {
                if button(ui, t, "キャンセル").clicked() {
                    capturing = false;
                }
                ui.label(
                    RichText::new(hint.unwrap_or("キーを押してください…"))
                        .size(t.fs_small)
                        .color(t.text_muted),
                );
            } else {
                if button(ui, t, "変更").clicked() {
                    capturing = true;
                }
                ui.label(RichText::new(&current).color(t.text));
            }
        },
    );
    ctx.data_mut(|d| d.insert_temp(id, capturing));
    g.wrapped_row(|ui| {
        ui.add_enabled_ui(!cfg.general.pause_shortcut.is_empty(), |ui| {
            if button(ui, t, "クリア").clicked() {
                cfg.general.pause_shortcut.clear();
            }
        });
        ui.add_enabled_ui(
            cfg.general.pause_shortcut != crate::config::DEFAULT_PAUSE_SHORTCUT,
            |ui| {
                if button(ui, t, "既定に戻す").clicked() {
                    cfg.general.pause_shortcut = crate::config::DEFAULT_PAUSE_SHORTCUT.to_owned();
                }
            },
        );
        ui.label(
            RichText::new(shortcut_note(&cfg.general.pause_shortcut, shortcut))
                .size(t.fs_small)
                .color(if shortcut == ShortcutStatus::Conflict {
                    t.text
                } else {
                    t.text_muted
                }),
        );
    });
    g.toggle_row(
        None,
        "全画面のアプリの間は自動で一時停止",
        "ゲームなど、全画面で動くアプリが前面にある間は、自動で一時停止します。既定はオフです。",
        &mut cfg.general.pause_in_fullscreen,
    );
}

/// "動作中", "一時停止中" or "一時停止中（あと 12 分）".
fn pause_state_text(pause: PauseView) -> String {
    match (pause.paused, pause.remaining) {
        (false, _) => "動作中".to_owned(),
        (true, Some(left)) => format!("一時停止中（{}）", remaining_label(left)),
        (true, None) if pause.manual => "一時停止中".to_owned(),
        (true, None) => "一時停止中（全画面のアプリが前面にあります）".to_owned(),
    }
}

/// What to tell the user about the shortcut's registration.
fn shortcut_note(text: &str, status: ShortcutStatus) -> &'static str {
    match status {
        ShortcutStatus::Active => "登録しました。",
        ShortcutStatus::Conflict => {
            "ほかのアプリがこの組み合わせを使っているため、登録できません。別の組み合わせにしてください。"
        }
        ShortcutStatus::Invalid => "Ctrl か Alt を含む組み合わせにしてください。",
        ShortcutStatus::Off if text.is_empty() => "ショートカットは設定されていません。",
        ShortcutStatus::Off => "タスクトレイが使えないため、ショートカットは登録されていません。",
    }
}

enum Capture {
    Waiting,
    /// A key came without Ctrl or Alt.
    NeedsModifier,
    Cancel,
    Got(String),
}

/// The first key press of this frame, as a capture result.
fn capture_result(ctx: &egui::Context) -> Capture {
    ctx.input(|i| {
        for ev in &i.events {
            if let egui::Event::Key {
                key,
                pressed: true,
                repeat: false,
                modifiers,
                ..
            } = ev
            {
                if *key == Key::Escape && !modifiers.ctrl && !modifiers.alt && !modifiers.shift {
                    return Capture::Cancel;
                }
                return match shortcut_from_key(*key, *modifiers) {
                    Some(text) => Capture::Got(text),
                    None => Capture::NeedsModifier,
                };
            }
        }
        Capture::Waiting
    })
}

/// The config text of a pressed key with its modifiers (`Ctrl+Alt+Y`). `None` unless Ctrl or Alt
/// is held (a shortcut without one would take ordinary typing from every app) and the key is one
/// yubi can send.
pub fn shortcut_from_key(key: Key, mods: Modifiers) -> Option<String> {
    use std::str::FromStr;
    if !(mods.ctrl || mods.alt) {
        return None;
    }
    let name = match key.name() {
        n if n.len() == 1 && n.chars().all(|c| c.is_ascii_alphanumeric()) => n.to_owned(),
        n if n.len() >= 2 && n.starts_with('F') && n[1..].chars().all(|c| c.is_ascii_digit()) => {
            n.to_owned()
        }
        _ => match key {
            Key::ArrowLeft => "Left",
            Key::ArrowRight => "Right",
            Key::ArrowUp => "Up",
            Key::ArrowDown => "Down",
            Key::Space => "Space",
            Key::Tab => "Tab",
            Key::Enter => "Enter",
            Key::Backspace => "Backspace",
            Key::Delete => "Delete",
            Key::Insert => "Insert",
            Key::Home => "Home",
            Key::End => "End",
            Key::PageUp => "PageUp",
            Key::PageDown => "PageDown",
            _ => return None,
        }
        .to_owned(),
    };
    let mut parts = Vec::new();
    if mods.ctrl {
        parts.push("Ctrl");
    }
    if mods.alt {
        parts.push("Alt");
    }
    if mods.shift {
        parts.push("Shift");
    }
    parts.push(&name);
    let text = parts.join("+");
    crate::os::keys::KeyCombo::from_str(&text).ok()?;
    Some(text)
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
        ui.add_enabled_ui(st.can_undo() && !st.io_busy && !st.read_only, |ui| {
            if button(ui, t, "元に戻す")
                .on_hover_text(
                    "読み込み前の状態にすべての設定を戻します。その後の変更は失われます。",
                )
                .clicked()
            {
                st.undo_import(cfg, config_path);
            }
        });
        if st.read_only {
            ui.label(
                RichText::new(READ_ONLY_REASON)
                    .size(t.fs_small)
                    .color(t.text_muted),
            );
        } else if st.can_undo() && !st.io_busy {
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
            ui.add_enabled_ui(changed && !st.read_only, |ui| {
                if accent_button(ui, t, "適用").clicked() {
                    st.apply_preview(cfg, config_path);
                }
            });
            if st.read_only {
                ui.label(
                    RichText::new(READ_ONLY_REASON)
                        .size(t.fs_small)
                        .color(t.text_muted),
                );
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn mods(ctrl: bool, alt: bool, shift: bool) -> Modifiers {
        Modifiers {
            ctrl,
            alt,
            shift,
            ..Modifiers::NONE
        }
    }

    #[test]
    fn captured_keys_become_config_text() {
        assert_eq!(
            shortcut_from_key(Key::Y, mods(true, true, false)).as_deref(),
            Some("Ctrl+Alt+Y")
        );
        assert_eq!(
            shortcut_from_key(Key::F5, mods(false, true, true)).as_deref(),
            Some("Alt+Shift+F5")
        );
        assert_eq!(
            shortcut_from_key(Key::Num7, mods(true, false, false)).as_deref(),
            Some("Ctrl+7")
        );
        assert_eq!(
            shortcut_from_key(Key::ArrowLeft, mods(true, false, false)).as_deref(),
            Some("Ctrl+Left")
        );
    }

    #[test]
    fn a_shortcut_needs_ctrl_or_alt_and_a_known_key() {
        assert_eq!(shortcut_from_key(Key::Y, mods(false, false, false)), None);
        assert_eq!(shortcut_from_key(Key::Y, mods(false, false, true)), None);
        assert_eq!(
            shortcut_from_key(Key::Escape, mods(true, false, false)),
            None
        );
    }

    #[test]
    fn the_pause_state_reads_naturally() {
        use std::time::Duration;
        let view = |paused, manual, remaining| PauseView {
            paused,
            manual,
            remaining,
        };
        assert_eq!(pause_state_text(view(false, false, None)), "動作中");
        assert_eq!(pause_state_text(view(true, true, None)), "一時停止中");
        assert_eq!(
            pause_state_text(view(true, true, Some(Duration::from_secs(61)))),
            "一時停止中（あと 2 分）"
        );
        assert!(pause_state_text(view(true, false, None)).contains("全画面"));
    }

    #[test]
    fn every_status_has_a_note() {
        for s in [
            ShortcutStatus::Off,
            ShortcutStatus::Active,
            ShortcutStatus::Conflict,
            ShortcutStatus::Invalid,
        ] {
            assert!(!shortcut_note("Ctrl+Alt+Y", s).is_empty());
        }
        assert!(shortcut_note("", ShortcutStatus::Off).contains("設定されていません"));
    }
}
