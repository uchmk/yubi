//! アプリごとの割り当て: the list of per-app profiles and the editor of one profile's overrides.
//! Overrides are only offered for the gestures the resolver honours (see `overview::overridable_ids`).

use eframe::egui::{self, Align, Layout, RichText, Ui};

use super::Cmd;
use super::features;
use super::overview::{feature_on_key, gesture_name, overridable_ids, page_of, profile_title};
use crate::config::{
    Action, ActionKind, Config, Cursor, CustomEdge, EDGE_KEYS, EdgeKind, EdgeTarget, Profile,
    Scroll,
};
use crate::engine::resolve::{basename, profile_matches};
use crate::howto;
use crate::icons;
use crate::nav::Page;
use crate::theme::Tokens;
use crate::widgets::{
    self, SliderSpec, accent_button, button, combo_menu, combo_sized, group, icon_button,
    section_header, step_field,
};

// ---------------------------------------------------------------------------------------------
// Pure helpers

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExeError {
    Empty,
    /// The exe (as the existing profile spells it) is already registered.
    Duplicate(String),
}

impl ExeError {
    pub fn message(&self) -> String {
        match self {
            Self::Empty => "実行ファイル名を入力してください。".to_owned(),
            Self::Duplicate(n) => format!("「{n}」はすでに登録されています。"),
        }
    }
}

/// The exe base name to store for `text`, or why it cannot be used. `except` is the profile being
/// renamed (it may keep its own name). Case-insensitive, like the resolver.
pub fn check_exe(
    profiles: &[Profile],
    text: &str,
    except: Option<usize>,
) -> Result<String, ExeError> {
    let name = basename(text);
    if name.is_empty() {
        return Err(ExeError::Empty);
    }
    let candidate = Profile {
        exe_name: name.to_owned(),
        ..Profile::default()
    };
    for (i, p) in profiles.iter().enumerate() {
        if Some(i) != except && profile_matches(&candidate, &p.exe_name) {
            return Err(ExeError::Duplicate(basename(&p.exe_name).to_owned()));
        }
    }
    Ok(name.to_owned())
}

/// What a row of the editor offers: keep the global action, or an action kind of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    Inherit,
    Kind(ActionKind),
}

impl Choice {
    fn label(self) -> &'static str {
        match self {
            Self::Inherit => "全体と同じ",
            Self::Kind(k) => k.label(),
        }
    }

    #[cfg(test)]
    fn options() -> Vec<Self> {
        std::iter::once(Self::Inherit)
            .chain(ActionKind::ALL.into_iter().map(Self::Kind))
            .collect()
    }

    pub fn of(over: Option<&Action>) -> Self {
        over.map_or(Self::Inherit, |a| Self::Kind(a.kind()))
    }
}

/// Apply a pick of the action combo to `id` of `p`.
pub fn apply_choice(p: &mut Profile, id: &str, choice: Choice) {
    match choice {
        Choice::Inherit => {
            p.overrides.remove(id);
        }
        Choice::Kind(k) => p.overrides.entry(id.to_owned()).or_default().set_kind(k),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeChoice {
    Inherit,
    Kind(EdgeKind),
}

impl EdgeChoice {
    fn label(self) -> &'static str {
        match self {
            Self::Inherit => "全体と同じ",
            Self::Kind(k) => k.label(),
        }
    }

    fn options() -> Vec<Self> {
        std::iter::once(Self::Inherit)
            .chain(EdgeKind::ALL.into_iter().map(Self::Kind))
            .collect()
    }

    pub fn of(over: Option<&EdgeTarget>) -> Self {
        over.map_or(Self::Inherit, |a| Self::Kind(a.kind()))
    }
}

pub fn apply_edge_choice(p: &mut Profile, key: &str, choice: EdgeChoice) {
    match choice {
        EdgeChoice::Inherit => {
            p.edge_overrides.remove(key);
        }
        EdgeChoice::Kind(k) => p
            .edge_overrides
            .entry(key.to_owned())
            .or_default()
            .set_kind(k),
    }
}

/// How many gestures and edges the profile replaces.
pub fn override_count(p: &Profile) -> usize {
    p.overrides.len()
        + p.edge_overrides.len()
        + usize::from(p.cursor_speed.is_some())
        + usize::from(p.scroll_speed.is_some())
}

// ---------------------------------------------------------------------------------------------
// State and page

#[derive(Debug, Default)]
pub struct AppsState {
    /// The profile whose detail is open.
    pub selected: Option<usize>,
    add_text: String,
    add_error: Option<String>,
    /// Running apps' exe names, read when the list opens and on 更新.
    running: Option<Vec<String>>,
    /// The profile removed last, for 元に戻す.
    deleted: Option<(usize, Profile)>,
    undo_error: Option<String>,
    /// Edit buffer of the open profile's exe name, and the profile it belongs to.
    exe_buf: String,
    exe_for: Option<usize>,
    exe_error: Option<String>,
}

pub fn page(ui: &mut Ui, t: &Tokens, cfg: &mut Config, st: &mut AppsState) -> Option<Cmd> {
    match st.selected {
        Some(i) if i < cfg.profiles.len() => detail(ui, t, cfg, st, i),
        _ => {
            st.selected = None;
            list(ui, t, cfg, st);
        }
    }
    None
}

fn refresh_running(st: &mut AppsState) {
    st.running = Some(crate::os::running::running_exe_names());
}

fn list(ui: &mut Ui, t: &Tokens, cfg: &mut Config, st: &mut AppsState) {
    widgets::page_header(ui, t, Page::Profiles.label(), Page::Profiles.subtitle());
    if st.running.is_none() {
        refresh_running(st);
    }

    // Undo bar.
    let mut undo = false;
    let mut dismiss = false;
    if let Some((_, p)) = &st.deleted {
        let name = profile_title(p);
        group(ui, t, |g| {
            g.wrapped_row(|ui| {
                ui.label(RichText::new(format!("「{name}」を削除しました。")).size(t.fs_body));
                if button(ui, t, "元に戻す").clicked() {
                    undo = true;
                }
                if button(ui, t, "閉じる").clicked() {
                    dismiss = true;
                }
            });
        });
    }
    if let Some(msg) = &st.undo_error {
        ui.label(RichText::new(msg).size(t.fs_small).color(t.accent));
    }
    if undo && let Some((i, p)) = st.deleted.take() {
        match check_exe(&cfg.profiles, &p.exe_name, None) {
            Ok(_) => {
                let at = i.min(cfg.profiles.len());
                cfg.profiles.insert(at, p);
                st.undo_error = None;
            }
            Err(e) => {
                st.undo_error = Some(format!("元に戻せませんでした。{}", e.message()));
                // Keep it so the user can fix the clash and try again.
                st.deleted = Some((i, p));
            }
        }
    }
    if dismiss {
        st.deleted = None;
        st.undo_error = None;
    }

    // The list.
    section_header(ui, t, "登録したアプリ");
    let mut open = None;
    let mut remove = None;
    group(ui, t, |g| {
        for (i, p) in cfg.profiles.iter().enumerate() {
            let title = profile_title(p);
            let mut desc = String::new();
            if !p.display_name.trim().is_empty() {
                desc.push_str(p.exe_name.trim());
                desc.push_str(" ・ ");
            }
            if p.enabled {
                desc.push_str(&format!("上書き {} 件", override_count(p)));
            } else {
                desc.push_str("yubi を止めています");
            }
            g.row(Some(icons::APPS), &title, &desc, |ui| {
                if icon_button(ui, t, icons::CHEVRON_RIGHT, "割り当てを編集").clicked() {
                    open = Some(i);
                }
                if icon_button(ui, t, icons::DELETE, "削除").clicked() {
                    remove = Some(i);
                }
            });
        }
        if cfg.profiles.is_empty() {
            g.wrapped_row(|ui| {
                ui.vertical(|ui| {
                    howto::two_col(
                        ui,
                        t,
                        howto::strip_width(3, howto::CARD_W_MAX),
                        |ui| {
                            howto::steps(
                                ui,
                                t,
                                &howto::app_step_kinds(),
                                &[
                                    "アプリを追加",
                                    "変えたい項目を選ぶ",
                                    "そのアプリの中だけで有効",
                                ],
                                None,
                            );
                        },
                        |ui| {
                            howto::summary(
                                ui,
                                t,
                                "アプリはまだ登録されていません",
                                &[
                                    "アプリごとの設定は、そのアプリが前面にある間だけ、全体の設定を差し替えます。".to_owned(),
                                    "下の「アプリを追加」に実行ファイル名（例: code.exe）を入力して「追加」を押すか、「実行中のアプリから選ぶ」で選びます。".to_owned(),
                                    "差し替えられるもの: 各ジェスチャーの動作、エッジスライダーの対象、カーソルの速度、2 本指スクロールの速さ、そのアプリでの yubi の停止。".to_owned(),
                                    "例: code.exe の左端を「スクロール」にすると、VS Code の前面にいるときだけ左端でスクロールできます。".to_owned(),
                                ],
                            );
                        },
                    );
                });
            });
        }
    });
    if let Some(i) = open {
        st.selected = Some(i);
    }
    if let Some(i) = remove {
        let p = cfg.profiles.remove(i);
        st.deleted = Some((i, p));
        st.undo_error = None;
    }

    // Add.
    section_header(ui, t, "アプリを追加");
    let mut add = false;
    let mut pick = None;
    let mut reload = false;
    group(ui, t, |g| {
        g.wrapped_row(|ui| {
            let r = ui.add(
                egui::TextEdit::singleline(&mut st.add_text)
                    .hint_text("実行ファイル名（例: code.exe）")
                    .desired_width(t.text_input_w),
            );
            if r.changed() {
                st.add_error = None;
            }
            let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if accent_button(ui, t, "追加").clicked() || enter {
                add = true;
            }
        });
        g.wrapped_row(|ui| {
            let names: Vec<&String> = st
                .running
                .iter()
                .flatten()
                .filter(|n| check_exe(&cfg.profiles, n, None).is_ok())
                .collect();
            combo_menu(
                ui,
                t,
                t.text_input_w,
                "running-apps",
                "実行中のアプリから選ぶ",
                |ui| {
                    if names.is_empty() {
                        ui.label(RichText::new("追加できるアプリはありません").color(t.text_muted));
                    }
                    for n in names {
                        if ui.selectable_label(false, n.as_str()).clicked() {
                            pick = Some(n.clone());
                        }
                    }
                },
            );
            if button(ui, t, "一覧を更新").clicked() {
                reload = true;
            }
        });
    });
    if let Some(n) = pick {
        st.add_text = n;
        st.add_error = None;
    }
    if reload {
        refresh_running(st);
    }
    if add {
        match check_exe(&cfg.profiles, &st.add_text, None) {
            Ok(name) => {
                cfg.profiles.push(Profile {
                    exe_name: name,
                    enabled: true,
                    ..Profile::default()
                });
                st.selected = Some(cfg.profiles.len() - 1);
                st.add_text.clear();
                st.add_error = None;
            }
            Err(e) => st.add_error = Some(e.message()),
        }
    }
    if let Some(msg) = &st.add_error {
        ui.label(RichText::new(msg).size(t.fs_small).color(t.accent));
    }
}

fn detail(ui: &mut Ui, t: &Tokens, cfg: &mut Config, st: &mut AppsState, i: usize) {
    if st.exe_for != Some(i) {
        st.exe_buf = cfg.profiles[i].exe_name.clone();
        st.exe_for = Some(i);
        st.exe_error = None;
    }
    ui.add_space(t.space_md);
    if button(ui, t, "アプリの一覧に戻る").clicked() {
        st.selected = None;
        st.exe_for = None;
        return;
    }
    let title = profile_title(&cfg.profiles[i]);
    widgets::page_header(
        ui,
        t,
        &title,
        "このアプリを前面にしている間だけ、操作を差し替えます。",
    );

    // The app itself.
    section_header(ui, t, "アプリ");
    let mut typed = false;
    let mut blocked = !cfg.profiles[i].enabled;
    group(ui, t, |g| {
        g.row(
            None,
            "実行ファイル名",
            "大文字・小文字は区別しません。",
            |ui| {
                let r = ui.add(
                    egui::TextEdit::singleline(&mut st.exe_buf)
                        .hint_text("例: code.exe")
                        .desired_width(t.text_input_w),
                );
                typed = r.changed();
            },
        );
        if let Some(msg) = &st.exe_error {
            g.raw_row(|ui| {
                ui.label(RichText::new(msg).size(t.fs_small).color(t.accent));
            });
        }
        g.row(
            None,
            "表示名",
            "一覧に表示する名前です（省略できます）。",
            |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut cfg.profiles[i].display_name)
                        .hint_text("例: VS Code")
                        .desired_width(t.text_input_w),
                );
            },
        );
        g.toggle_row(
            None,
            "このアプリでは yubi を止める",
            "オンにすると、このアプリが前面にある間は、すべてのジェスチャーを無視します。",
            &mut blocked,
        );
    });
    cfg.profiles[i].enabled = !blocked;
    if typed {
        match check_exe(&cfg.profiles, &st.exe_buf, Some(i)) {
            Ok(name) => {
                cfg.profiles[i].exe_name = name;
                st.exe_error = None;
            }
            Err(e) => st.exe_error = Some(e.message()),
        }
    }

    // Everything the global pages know, read before the profile is borrowed mutably.
    struct Info {
        id: String,
        name: String,
        page: Page,
        global: String,
        on: bool,
    }
    let infos: Vec<Info> = overridable_ids(cfg)
        .into_iter()
        .map(|id| {
            let page = page_of(id.as_str());
            Info {
                name: gesture_name(id.as_str()),
                global: cfg.binding(&id).map(|a| a.label()).unwrap_or_default(),
                on: feature_on_key(cfg, id.as_str()),
                id: id.as_str().to_owned(),
                page,
            }
        })
        .collect();
    let edges: Vec<(&str, String, bool)> = EDGE_KEYS
        .iter()
        .map(|k| {
            let target = match *k {
                "edge.left" => &cfg.edge.left,
                "edge.right" => &cfg.edge.right,
                _ => &cfg.edge.top,
            };
            (*k, target.label().to_owned(), cfg.edge.enabled)
        })
        .collect();

    let global_speed = cfg.cursor.speed;
    let scroll_on = cfg.scroll.enabled;
    let p = &mut cfg.profiles[i];
    let count = override_count(p);
    ui.add_enabled_ui(!blocked, |ui| {
        if blocked {
            ui.label(
                RichText::new("止めている間は、下の設定は使われません。")
                    .size(t.fs_small)
                    .color(t.text_muted),
            );
        }
        section_header(ui, t, "カーソル");
        group(ui, t, |g| cursor_row(g, t, p, global_speed));
        section_header(ui, t, "スクロール");
        group(ui, t, |g| scroll_speed_row(g, t, p, scroll_on));

        type Pick<'a> = Box<dyn Fn(&Info) -> bool + 'a>;
        let on_page = |page: Page, five: Option<bool>| -> Pick<'static> {
            Box::new(move |x: &Info| {
                x.page == page && five.is_none_or(|f| (features::tab_of_key(&x.id) == Some(1)) == f)
            })
        };
        let sections: [(&str, Pick); 3] = [
            ("4本指ジェスチャー", on_page(Page::Gesture, Some(false))),
            ("5本指ジェスチャー", on_page(Page::Gesture, Some(true))),
            (Page::CustomPress.label(), on_page(Page::CustomPress, None)),
        ];
        for (title, pick) in &sections {
            let rows: Vec<&Info> = infos.iter().filter(|x| pick(x)).collect();
            if rows.is_empty() {
                continue;
            }
            section_header(ui, t, title);
            group(ui, t, |g| {
                for info in rows {
                    let mut desc = format!("全体: {}", info.global);
                    if !info.on {
                        desc.push_str("（この機能は全体でオフです）");
                    }
                    g.row(None, &info.name, &desc, |ui| {
                        let mut choice = Choice::of(p.overrides.get(&info.id));
                        if let Some(pick) = widgets::kind_picker(
                            ui,
                            t,
                            t.combo_w,
                            ("profile-action", i, &info.id),
                            choice.label(),
                            match choice {
                                Choice::Inherit => None,
                                Choice::Kind(k) => Some(k),
                            },
                            Some(Choice::Inherit.label()),
                            if info.id.starts_with("pinch.") {
                                &ActionKind::PINCH
                            } else {
                                &ActionKind::ALL
                            },
                            ActionKind::label,
                        ) {
                            choice = match pick {
                                widgets::Pick::Inherit => Choice::Inherit,
                                widgets::Pick::Kind(k) => Choice::Kind(k),
                            };
                            apply_choice(p, &info.id, choice);
                        }
                        match p.overrides.get_mut(&info.id) {
                            Some(Action::Keys(keys)) => {
                                ui.add(
                                    egui::TextEdit::singleline(keys)
                                        .hint_text("例: Ctrl+Alt+T")
                                        .desired_width(t.text_input_w),
                                );
                            }
                            Some(Action::VolumeStep(n) | Action::BrightnessStep(n)) => {
                                step_field(ui, n)
                            }
                            Some(a @ Action::WindowArrange(_)) => {
                                widgets::action_fields(ui, t, &("profile-arrange", i, &info.id), a);
                            }
                            _ => {}
                        }
                    });
                }
            });
        }

        section_header(ui, t, Page::Edge.label());
        let edge_options = EdgeChoice::options();
        group(ui, t, |g| {
            for (key, global, on) in &edges {
                let mut desc = format!("全体: {global}");
                if !on {
                    desc.push_str("（この機能は全体でオフです）");
                }
                g.row(None, &gesture_name(key), &desc, |ui| {
                    let mut choice = EdgeChoice::of(p.edge_overrides.get(*key));
                    if combo_sized(
                        ui,
                        t,
                        t.combo_w * 1.6,
                        ("profile-edge", i, *key),
                        &mut choice,
                        &edge_options,
                        EdgeChoice::label,
                    ) {
                        apply_edge_choice(p, key, choice);
                    }
                });
                if let Some(EdgeTarget::Custom(c)) = p.edge_overrides.get_mut(*key) {
                    custom_edge_rows(g, t, c);
                }
            }
        });

        if count > 0 {
            ui.add_space(t.space_sm);
            if button(
                ui,
                t,
                &format!("{count} 件の上書きをすべて「全体と同じ」に戻す"),
            )
            .clicked()
            {
                p.overrides.clear();
                p.edge_overrides.clear();
                p.cursor_speed = None;
                p.scroll_speed = None;
            }
        }
    });
}

/// The per-app cursor speed: 「全体と同じ」 or a value of its own. It only applies while yubi
/// manages the cursor speed globally (the resolver returns nothing otherwise).
fn cursor_row(g: &mut widgets::Group<'_>, t: &Tokens, p: &mut Profile, global: Option<u8>) {
    let support = crate::os::touchpad::support();
    let usable = support.is_ok();
    let managed = global.is_some();
    let desc = match (&support, global) {
        (Err(e), _) => e.reason().to_owned(),
        (Ok(()), Some(v)) => format!("全体: {v}"),
        (Ok(()), None) => "全体: 管理していません".to_owned(),
    };
    g.row(None, "カーソルの速度", &desc, |ui| {
        ui.add_enabled_ui(usable && managed, |ui| {
            let mut choice = SpeedChoice::of(p.cursor_speed);
            if combo_sized(
                ui,
                t,
                t.combo_w,
                "profile-cursor-speed",
                &mut choice,
                &[SpeedChoice::Inherit, SpeedChoice::Own],
                SpeedChoice::label,
            ) {
                apply_speed_choice(p, choice == SpeedChoice::Own, global);
            }
        });
    });
    if let Some(v) = p.cursor_speed.as_mut() {
        let (lo, hi) = Cursor::RANGE;
        let spec = SliderSpec::int(u16::from(lo), u16::from(hi), 1, "");
        g.row(None, "このアプリの値", "", |ui| {
            ui.add_enabled_ui(usable && managed, |ui| {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    // The first widget added sits rightmost.
                    ui.label(RichText::new(v.to_string()).size(t.fs_body));
                    ui.add_space(t.space_sm);
                    ui.label(RichText::new("速い").size(t.fs_small).color(t.text_muted));
                    let mut x = f64::from(*v);
                    if widgets::slider(ui, t, &mut x, spec) {
                        *v = <u8 as widgets::Num>::from_f64(x);
                    }
                    ui.label(RichText::new("遅い").size(t.fs_small).color(t.text_muted));
                });
            });
        });
    }
    if usable && !managed {
        g.wrapped_row(|ui| {
            ui.label(
                RichText::new(
                    "アプリごとの速さは、全体で「yubi でカーソルの速度を管理する」がオンのときだけ使われます。",
                )
                .size(t.fs_small)
                .color(t.text_muted),
            );
        });
    }
}

/// The per-app two-finger scroll speed: 「全体と同じ」 or a multiplier of its own, applied on top
/// of the global speeds. It only applies while yubi's two-finger scroll is on.
fn scroll_speed_row(g: &mut widgets::Group<'_>, t: &Tokens, p: &mut Profile, scroll_on: bool) {
    let desc = if scroll_on {
        "全体の速さに掛ける倍率です。".to_owned()
    } else {
        "全体で 2 本指スクロールを使うがオフのため、使われません。".to_owned()
    };
    g.row(None, "スクロールの速度", &desc, |ui| {
        ui.add_enabled_ui(scroll_on, |ui| {
            let mut choice = SpeedChoice::of(p.scroll_speed);
            if combo_sized(
                ui,
                t,
                t.combo_w,
                "profile-scroll-speed",
                &mut choice,
                &[SpeedChoice::Inherit, SpeedChoice::Own],
                SpeedChoice::label,
            ) {
                apply_scroll_choice(p, choice == SpeedChoice::Own);
            }
        });
    });
    if let Some(v) = p.scroll_speed.as_mut() {
        let r = Scroll::SPEED;
        let spec = SliderSpec {
            min: f64::from(r.min),
            max: f64::from(r.max),
            step: 0.1,
            decimals: 1,
            suffix: " 倍",
        };
        g.row(None, "このアプリの値", "", |ui| {
            ui.add_enabled_ui(scroll_on, |ui| {
                widgets::slider_with_value(ui, t, v, spec);
            });
        });
    }
}

/// Switch the profile between 「全体と同じ」 and a multiplier of its own (starting at 1.0).
fn apply_scroll_choice(p: &mut Profile, own: bool) {
    p.scroll_speed = own.then_some(1.0);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SpeedChoice {
    Inherit,
    Own,
}

impl SpeedChoice {
    fn of<T>(v: Option<T>) -> Self {
        if v.is_some() {
            Self::Own
        } else {
            Self::Inherit
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Inherit => "全体と同じ",
            Self::Own => "このアプリの値",
        }
    }
}

/// Switch the profile between 「全体と同じ」 and a value of its own (starting at the global one).
fn apply_speed_choice(p: &mut Profile, own: bool, global: Option<u8>) {
    p.cursor_speed =
        own.then(|| Cursor::clamp_speed(global.unwrap_or(crate::os::touchpad::SPEED_DEFAULT)));
}

fn custom_edge_rows(g: &mut widgets::Group<'_>, t: &Tokens, c: &mut CustomEdge) {
    for (title, text, hint) in [
        ("進む方向のキー", &mut c.forward, "例: Ctrl+Tab"),
        ("戻る方向のキー", &mut c.back, "例: Ctrl+Shift+Tab"),
    ] {
        g.row(None, title, "", |ui| {
            ui.add(
                egui::TextEdit::singleline(text)
                    .hint_text(hint)
                    .desired_width(t.text_input_w),
            );
        });
    }
    g.slider_row(
        "1ステップの距離",
        "",
        SliderSpec {
            min: 1.0,
            max: 6.0,
            step: 0.5,
            decimals: 1,
            suffix: " mm",
        },
        &mut c.step_mm,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(exe: &str) -> Profile {
        Profile {
            exe_name: exe.to_owned(),
            enabled: true,
            ..Profile::default()
        }
    }

    #[test]
    fn exe_names_are_trimmed_to_the_base_name() {
        let none: Vec<Profile> = Vec::new();
        assert_eq!(
            check_exe(&none, "  Code.EXE ", None).as_deref(),
            Ok("Code.EXE")
        );
        assert_eq!(
            check_exe(&none, r"C:\Apps\code.exe", None).as_deref(),
            Ok("code.exe")
        );
        assert_eq!(check_exe(&none, "   ", None), Err(ExeError::Empty));
        assert_eq!(check_exe(&none, r"C:\Apps\", None), Err(ExeError::Empty));
    }

    #[test]
    fn duplicates_are_refused_case_insensitively() {
        let ps = vec![profile("code.exe"), profile("notepad.exe")];
        assert_eq!(
            check_exe(&ps, "CODE.EXE", None),
            Err(ExeError::Duplicate("code.exe".into()))
        );
        assert!(check_exe(&ps, r"D:\x\Code.exe", None).is_err());
        assert!(check_exe(&ps, "code2.exe", None).is_ok());
        // A profile may keep (or re-case) its own name.
        assert!(check_exe(&ps, "Code.exe", Some(0)).is_ok());
        assert!(check_exe(&ps, "notepad.exe", Some(0)).is_err());
        assert!(
            ExeError::Duplicate("code.exe".into())
                .message()
                .contains("code.exe")
        );
    }

    #[test]
    fn picking_an_action_creates_an_override_and_inherit_removes_it() {
        let mut p = profile("a.exe");
        apply_choice(&mut p, "swipe.4.left", Choice::Kind(ActionKind::Find));
        assert_eq!(p.overrides.get("swipe.4.left"), Some(&Action::Find));
        assert_eq!(
            Choice::of(p.overrides.get("swipe.4.left")),
            Choice::Kind(ActionKind::Find)
        );
        // Picking the same kind again keeps the payload.
        p.overrides
            .insert("tap.4".into(), Action::Keys("Ctrl+W".into()));
        apply_choice(&mut p, "tap.4", Choice::Kind(ActionKind::Keys));
        assert_eq!(
            p.overrides.get("tap.4"),
            Some(&Action::Keys("Ctrl+W".into()))
        );
        // "なし" is an explicit override (the gesture is off in this app).
        apply_choice(&mut p, "swipe.4.left", Choice::Kind(ActionKind::None));
        assert_eq!(p.overrides.get("swipe.4.left"), Some(&Action::None));
        apply_choice(&mut p, "swipe.4.left", Choice::Inherit);
        assert!(!p.overrides.contains_key("swipe.4.left"));
        assert_eq!(Choice::of(None), Choice::Inherit);
        assert_eq!(override_count(&p), 1);
    }

    #[test]
    fn edge_overrides_follow_the_same_rules() {
        let mut p = profile("a.exe");
        apply_edge_choice(&mut p, "edge.left", EdgeChoice::Kind(EdgeKind::Scroll));
        assert_eq!(p.edge_overrides.get("edge.left"), Some(&EdgeTarget::Scroll));
        apply_edge_choice(&mut p, "edge.left", EdgeChoice::Inherit);
        assert!(p.edge_overrides.is_empty());
        assert_eq!(EdgeChoice::of(None), EdgeChoice::Inherit);
    }

    #[test]
    fn the_scroll_speed_choice_starts_at_one() {
        let mut p = profile("a.exe");
        apply_scroll_choice(&mut p, true);
        assert_eq!(p.scroll_speed, Some(1.0));
        assert_eq!(override_count(&p), 1);
        apply_scroll_choice(&mut p, false);
        assert_eq!(p.scroll_speed, None);
        assert_eq!(override_count(&p), 0);
    }

    #[test]
    fn the_cursor_speed_choice_starts_at_the_global_value() {
        let mut p = profile("a.exe");
        apply_speed_choice(&mut p, true, Some(14));
        assert_eq!(p.cursor_speed, Some(14));
        assert_eq!(override_count(&p), 1);
        apply_speed_choice(&mut p, false, Some(14));
        assert_eq!(p.cursor_speed, None);
        assert_eq!(SpeedChoice::of::<u8>(None), SpeedChoice::Inherit);
    }

    #[test]
    fn the_choice_lists_start_with_inherit() {
        assert_eq!(Choice::options()[0], Choice::Inherit);
        assert_eq!(Choice::options().len(), ActionKind::ALL.len() + 1);
        assert_eq!(EdgeChoice::options()[0], EdgeChoice::Inherit);
        assert_eq!(Choice::Inherit.label(), "全体と同じ");
    }

    #[test]
    fn offered_overrides_survive_the_loader() {
        // Every id the editor offers must be one the config loader keeps.
        let cfg = Config::default();
        let mut p = profile("a.exe");
        for id in overridable_ids(&cfg) {
            apply_choice(&mut p, id.as_str(), Choice::Kind(ActionKind::Find));
        }
        let mut c = cfg.clone();
        c.profiles.push(p.clone());
        let text = toml::to_string_pretty(&c).expect("serialise");
        let parsed = crate::config::parse_text(&text).expect("parse");
        assert_eq!(parsed.config.profiles[0].overrides, p.overrides);
        assert!(parsed.unbindable_overrides.is_empty() && parsed.inert_overrides.is_empty());
    }
}
