//! 割り当ての一覧: every assignment in one table (gesture x global action x per-app overrides),
//! with the places where an assignment cannot work as set. The table and the conflict list come
//! from the pure `build`; the page only draws them.

use eframe::egui::{self, Align, Label, Layout, Pos2, Rect, RichText, Sense, Stroke, Ui, Vec2};

use super::Cmd;
use crate::config::{Action, ActionKind, Config, EDGE_KEYS, Profile};
use crate::engine::types::GestureId;
use crate::nav::Page;
use crate::theme::Tokens;
use crate::widgets::{self, group, section_header};
use crate::win_gestures;

// ---------------------------------------------------------------------------------------------
// Names and places of gestures (shared with the per-app page)

const DIR_NAMES: [(&str, &str); 4] = [
    ("left", "左"),
    ("right", "右"),
    ("up", "上"),
    ("down", "下"),
];
const CORNER_NAMES: [(&str, &str); 4] = [
    ("tl", "左上"),
    ("tr", "右上"),
    ("bl", "左下"),
    ("br", "右下"),
];

/// Japanese name of a gesture or edge key (`swipe.4.left`, `edge.top`, ...). Unknown keys come
/// back unchanged.
pub fn gesture_name(key: &str) -> String {
    let parts: Vec<&str> = key.split('.').collect();
    let n = || parts.get(1).copied().unwrap_or("?");
    match parts.as_slice() {
        ["tap", _] => format!("{}本指でタップ", n()),
        ["swipe", _, d] => {
            let d = DIR_NAMES
                .iter()
                .find(|(t, _)| t == d)
                .map_or(*d, |(_, j)| *j);
            format!("{}本指で{d}にスワイプ", n())
        }
        ["drag", "flick", d] => {
            let d = DIR_NAMES
                .iter()
                .find(|(t, _)| t == d)
                .map_or(*d, |(_, j)| *j);
            format!("タイトルバーを{d}に払う")
        }
        ["pinch", _, "in"] => format!("{}本指でピンチイン", n()),
        ["pinch", _, "out"] => format!("{}本指でピンチアウト", n()),
        ["press", _, "light"] => format!("{}本指で軽く押し込む", n()),
        ["press", _, "deep"] => format!("{}本指で深く押し込む", n()),
        ["hold", "long", n] => format!("{n}本指で長押し"),
        ["hold", kind, finger, trigger] => {
            let owner = if *kind == "thumb" {
                "サムホールド"
            } else {
                "フィンガーホールド"
            };
            let finger = crate::config::Finger::ALL
                .iter()
                .find(|f| f.token() == *finger)
                .map_or(*finger, |f| f.label());
            let trigger = crate::config::HoldTrigger::ALL
                .iter()
                .find(|t| t.token() == *trigger)
                .map_or(*trigger, |t| t.label());
            format!("{owner}: {finger}を置いて{trigger}")
        }
        ["corner", c, kind] => {
            let c = CORNER_NAMES
                .iter()
                .find(|(t, _)| t == c)
                .map_or(*c, |(_, j)| *j);
            match crate::config::CornerSlot::ALL
                .iter()
                .find(|s| s.token() == *kind)
            {
                Some(crate::config::CornerSlot::Swipe(d)) => {
                    format!("{c}の角から{}にスワイプ", d.label())
                }
                Some(other) => format!("{c}の角で{}", other.label()),
                None => format!("{c}の角で{kind}"),
            }
        }
        ["edge", "left"] => "左端をなぞる".to_owned(),
        ["edge", "right"] => "右端をなぞる".to_owned(),
        ["edge", "top"] => "上端をなぞる".to_owned(),
        _ => key.to_owned(),
    }
}

/// The page that edits the gesture or edge key.
pub fn page_of(key: &str) -> Page {
    let parts: Vec<&str> = key.split('.').collect();
    match parts.as_slice() {
        ["tap" | "swipe" | "pinch", ..] => Page::Gesture,
        ["drag", ..] => Page::Drag,
        ["press", ..] => Page::CustomPress,
        ["hold", "long", _] => Page::LongHold,
        ["hold", "thumb", ..] => Page::ThumbHold,
        ["hold", ..] => Page::FingerHold,
        ["corner", ..] => Page::Corner,
        ["edge", ..] => Page::Edge,
        _ => Page::Home,
    }
}

/// Whether the feature page that owns `page` is switched on.
pub fn feature_on(cfg: &Config, page: Page) -> bool {
    match page {
        Page::CustomPress => cfg.press.enabled,
        Page::LongHold => cfg.long_hold.enabled,
        Page::FingerHold => cfg.finger_hold.enabled,
        Page::ThumbHold => cfg.thumb_hold.enabled,
        Page::Corner => cfg.corner.enabled,
        Page::Edge => cfg.edge.enabled,
        _ => true,
    }
}

/// Whether the feature that owns the gesture or edge `key` is switched on (the ジェスチャー page
/// has one switch per finger count).
pub fn feature_on_key(cfg: &Config, key: &str) -> bool {
    match key.split('.').collect::<Vec<_>>().as_slice() {
        ["drag", "flick", _] => cfg.drag.flick.enabled,
        ["tap" | "swipe" | "pinch", "3", ..] => cfg.three.enabled,
        ["tap" | "swipe" | "pinch", "5", ..] => cfg.five.enabled,
        ["tap" | "swipe" | "pinch", ..] => cfg.four.enabled,
        _ => feature_on(cfg, page_of(key)),
    }
}

/// The ids a per-app override can target: the ones the resolver honours (no `corner.*`, no
/// `hold.*`) and that a global field backs.
pub fn overridable_ids(cfg: &Config) -> Vec<GestureId> {
    GestureId::all()
        .into_iter()
        .filter(|id| {
            let s = id.as_str();
            !s.starts_with("corner.") && !s.starts_with("hold.") && cfg.binding(id).is_some()
        })
        .collect()
}

/// Finger count of a `tap` / `swipe` / `pinch` / `press` id.
fn fingers_of(key: &str) -> Option<u8> {
    let mut it = key.split('.');
    matches!(it.next()?, "tap" | "swipe" | "pinch" | "press")
        .then(|| it.next()?.parse().ok())
        .flatten()
}

// ---------------------------------------------------------------------------------------------
// The table

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cell {
    /// The app does not override this gesture.
    Inherit,
    /// The app replaces the action (`なし` turns the gesture off there).
    Set(String),
    /// yubi is switched off for the app.
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Column {
    /// Index into `Config::profiles`.
    pub profile: usize,
    /// Lowercased exe name: a stable key while profiles are added, removed or reordered.
    pub key: String,
    pub title: String,
    pub blocked: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// Gesture id or edge key.
    pub key: String,
    pub name: String,
    pub global: String,
    /// The page that edits it.
    pub page: Page,
    /// One cell per column.
    pub cells: Vec<Cell>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictKind {
    /// Two triggers the engine cannot tell apart.
    Overlap,
    /// A binding whose feature (or the whole gesture switch) is off.
    Disabled,
    /// A Windows built-in gesture that still acts first.
    WindowsWins,
}

impl ConflictKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Overlap => "重なり",
            Self::Disabled => "機能オフ",
            Self::WindowsWins => "Windows が先",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    pub kind: ConflictKind,
    /// Row keys it affects (may be empty for a setting that has no row).
    pub rows: Vec<String>,
    pub reason: String,
    /// Where to fix it.
    pub page: Page,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Table {
    pub columns: Vec<Column>,
    pub rows: Vec<Row>,
    pub conflicts: Vec<Conflict>,
}

impl Table {
    /// The conflict kinds that touch row `key`, without repeats.
    pub fn kinds_of(&self, key: &str) -> Vec<ConflictKind> {
        let mut out = Vec::new();
        for c in &self.conflicts {
            if c.rows.iter().any(|r| r == key) && !out.contains(&c.kind) {
                out.push(c.kind);
            }
        }
        out
    }
}

/// The name a profile goes by in lists.
pub fn profile_title(p: &Profile) -> String {
    let d = p.display_name.trim();
    if !d.is_empty() {
        d.to_owned()
    } else if p.exe_name.trim().is_empty() {
        "（名前なし）".to_owned()
    } else {
        p.exe_name.trim().to_owned()
    }
}

fn edge_label(t: &crate::config::EdgeTarget) -> String {
    t.kind().label().to_owned()
}

fn global_edge<'a>(cfg: &'a Config, key: &str) -> Option<&'a crate::config::EdgeTarget> {
    match key {
        "edge.left" => Some(&cfg.edge.left),
        "edge.right" => Some(&cfg.edge.right),
        "edge.top" => Some(&cfg.edge.top),
        _ => None,
    }
}

/// Build the table and the conflict list. `win` is the state of Windows' own gestures.
pub fn build(cfg: &Config, win: &win_gestures::Values) -> Table {
    // Columns: the apps that override something, or are switched off and have a name.
    let columns: Vec<Column> = cfg
        .profiles
        .iter()
        .enumerate()
        .filter(|(_, p)| !p.overrides.is_empty() || !p.edge_overrides.is_empty() || !p.enabled)
        .map(|(i, p)| Column {
            profile: i,
            key: p.exe_name.to_lowercase(),
            title: profile_title(p),
            blocked: !p.enabled,
        })
        .collect();

    let cells_for = |get: &dyn Fn(&Profile) -> Option<String>| -> Vec<Cell> {
        columns
            .iter()
            .map(|c| {
                let p = &cfg.profiles[c.profile];
                if !p.enabled {
                    Cell::Blocked
                } else {
                    get(p).map_or(Cell::Inherit, Cell::Set)
                }
            })
            .collect()
    };

    let mut rows = Vec::new();
    for id in GestureId::all() {
        let Some(global) = cfg.binding(&id) else {
            continue;
        };
        let key = id.as_str();
        let overridable = !key.starts_with("corner.") && !key.starts_with("hold.");
        let cells = if overridable {
            cells_for(&|p| p.overrides.get(key).map(Action::label))
        } else {
            cells_for(&|_| None)
        };
        let has_override = cells.iter().any(|c| matches!(c, Cell::Set(_)));
        if global.kind() == ActionKind::None && !has_override {
            continue;
        }
        rows.push(Row {
            key: key.to_owned(),
            name: gesture_name(key),
            global: global.label(),
            page: page_of(key),
            cells,
        });
    }
    for key in EDGE_KEYS {
        let Some(global) = global_edge(cfg, key) else {
            continue;
        };
        let cells = cells_for(&|p| p.edge_overrides.get(key).map(edge_label));
        let has_override = cells.iter().any(|c| matches!(c, Cell::Set(_)));
        if global.kind() == crate::config::EdgeKind::None && !has_override {
            continue;
        }
        rows.push(Row {
            key: key.to_owned(),
            name: gesture_name(key),
            global: edge_label(global),
            page: Page::Edge,
            cells,
        });
    }

    let conflicts = conflicts(cfg, win, &rows);
    Table {
        columns,
        rows,
        conflicts,
    }
}

/// A row has something to run: a global action, or an override that is not "none".
fn row_is_bound(row: &Row) -> bool {
    let none = ActionKind::None.label();
    row.global != none
        || row
            .cells
            .iter()
            .any(|c| matches!(c, Cell::Set(s) if s != none))
}

/// The drag and a long hold use the same finger count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DragHoldOverlap {
    pub fingers: u8,
    /// The long hold of that count opens the ring (otherwise it runs a plain action).
    pub ring: bool,
}

impl DragHoldOverlap {
    /// The explanation shown on the overview.
    pub fn reason(self) -> String {
        let n = self.fingers;
        if self.ring {
            format!(
                "{n}本指: ドラッグとロングホールド（リング）が同じ本数です。止めてから動かすとリングが先に開きます。どちらかの本数を変えてください。"
            )
        } else {
            format!(
                "{n}本指: ドラッグとロングホールドが同じ本数です。止めてから動かすとロングホールドが先に動きます。どちらかの本数を変えてください。"
            )
        }
    }

    /// The short note on the ドラッグ and ロングホールド pages.
    pub fn note(self) -> String {
        let n = self.fingers;
        let what = if self.ring {
            "ロングホールド（リング）"
        } else {
            "ロングホールド"
        };
        format!(
            "{n}本指がドラッグと{what}で重なっています。止めてから動かすと先に長押しが動きます。本数を変えると避けられます。"
        )
    }
}

/// The drag rows whose finger count a long hold that is switched on shares.
pub fn drag_hold_overlaps(cfg: &Config) -> Vec<DragHoldOverlap> {
    cfg.drag
        .active()
        .filter_map(|(count, _)| {
            let n = count as u8;
            if cfg.ring_counts().contains(&count) {
                return Some(DragHoldOverlap {
                    fingers: n,
                    ring: true,
                });
            }
            let a = cfg.long_hold.actions.get(count);
            (cfg.long_hold.enabled && *a != Action::None && *a != Action::RingMenu).then_some(
                DragHoldOverlap {
                    fingers: n,
                    ring: false,
                },
            )
        })
        .collect()
}

/// The first of [`drag_hold_overlaps`].
#[cfg(test)]
pub fn drag_hold_overlap(cfg: &Config) -> Option<DragHoldOverlap> {
    drag_hold_overlaps(cfg).into_iter().next()
}

fn conflicts(cfg: &Config, win: &win_gestures::Values, rows: &[Row]) -> Vec<Conflict> {
    let mut out = Vec::new();
    let bound: Vec<&Row> = rows.iter().filter(|r| row_is_bound(r)).collect();
    let keys = |pred: &dyn Fn(&Row) -> bool| -> Vec<String> {
        bound
            .iter()
            .filter(|r| pred(r))
            .map(|r| r.key.clone())
            .collect()
    };

    // The master switch.
    if !cfg.gestures_enabled && !bound.is_empty() {
        out.push(Conflict {
            kind: ConflictKind::Disabled,
            rows: keys(&|_| true),
            reason: "「ジェスチャー」のスイッチがオフのため、すべての割り当てが止まっています。"
                .into(),
            page: Page::Home,
        });
    }

    // Features that are off but still have bindings (one entry per page, and per finger count on
    // the ジェスチャー page).
    type Pick = Box<dyn Fn(&Row) -> bool>;
    let sections: [(Page, &str, Option<u8>, Pick); 9] = [
        (
            Page::Gesture,
            "3本指ジェスチャー",
            Some(3),
            Box::new(|r| r.page == Page::Gesture && fingers_of(&r.key) == Some(3)),
        ),
        (
            Page::Gesture,
            "4本指ジェスチャー",
            Some(4),
            Box::new(|r| r.page == Page::Gesture && fingers_of(&r.key) == Some(4)),
        ),
        (
            Page::Gesture,
            "5本指ジェスチャー",
            Some(5),
            Box::new(|r| r.page == Page::Gesture && fingers_of(&r.key) == Some(5)),
        ),
        (
            Page::CustomPress,
            "",
            None,
            Box::new(|r| r.page == Page::CustomPress),
        ),
        (
            Page::LongHold,
            "",
            None,
            Box::new(|r| r.page == Page::LongHold),
        ),
        (
            Page::FingerHold,
            "",
            None,
            Box::new(|r| r.page == Page::FingerHold),
        ),
        (
            Page::ThumbHold,
            "",
            None,
            Box::new(|r| r.page == Page::ThumbHold),
        ),
        (Page::Corner, "", None, Box::new(|r| r.page == Page::Corner)),
        (Page::Edge, "", None, Box::new(|r| r.page == Page::Edge)),
    ];
    for (page, title, count, pick) in &sections {
        let page = *page;
        let on = match count {
            Some(3) => cfg.three.enabled,
            Some(5) => cfg.five.enabled,
            Some(_) => cfg.four.enabled,
            None => feature_on(cfg, page),
        };
        if on {
            continue;
        }
        let affected = keys(&|r| pick(r));
        if !affected.is_empty() {
            let name = if title.is_empty() {
                page.label()
            } else {
                title
            };
            out.push(Conflict {
                kind: ConflictKind::Disabled,
                rows: affected,
                reason: format!(
                    "「{name}」がオフのため、この割り当ては動きません。ページで有効にしてください。"
                ),
                page,
            });
        }
    }

    // Windows' own gestures win over ours while they are on.
    for fingers in [3u8, 4] {
        if !win_gestures::conflicts(win, fingers) {
            continue;
        }
        let affected = keys(&|r| {
            fingers_of(&r.key) == Some(fingers)
                && !r.key.starts_with("press.")
                && feature_on_key(cfg, &r.key)
        });
        if !affected.is_empty() {
            out.push(Conflict {
                kind: ConflictKind::WindowsWins,
                rows: affected,
                reason: format!(
                    "Windows の{fingers}本指ジェスチャーが有効なままで、先に反応します。「全般」で無効にできます。"
                ),
                page: Page::General,
            });
        }
        if cfg.drag.on(usize::from(fingers)) {
            out.push(Conflict {
                kind: ConflictKind::WindowsWins,
                rows: Vec::new(),
                reason: format!(
                    "{fingers}本指のドラッグが、有効なままの Windows の{fingers}本指ジェスチャーと重なります。「全般」で無効にできます。"
                ),
                page: Page::General,
            });
        }
    }

    // Triggers the engine cannot tell apart.
    for (n, row) in cfg.drag.active() {
        if row.start == crate::config::DragStart::TouchMove {
            let prefix = format!("swipe.{n}.");
            let affected = keys(&|r| r.key.starts_with(&prefix) && feature_on_key(cfg, &r.key));
            if !affected.is_empty() {
                out.push(Conflict {
                    kind: ConflictKind::Overlap,
                    rows: affected,
                    reason: format!(
                        "ドラッグが同じ{n}本指を使うため、スワイプはドラッグとして先に判定されます。"
                    ),
                    page: Page::Drag,
                });
            }
        } else {
            let tap = format!("tap.{n}");
            let affected = keys(&|r| r.key == tap && feature_on_key(cfg, &r.key));
            if !affected.is_empty() {
                out.push(Conflict {
                    kind: ConflictKind::Overlap,
                    rows: affected,
                    reason: format!(
                        "{n}本指のタップは、「タップ後に動かす」の最初のタップと同じ操作です。動きが続かないと分かってから、少し遅れて実行されます。"
                    ),
                    page: Page::Drag,
                });
            }
        }
    }
    for o in drag_hold_overlaps(cfg) {
        out.push(Conflict {
            kind: ConflictKind::Overlap,
            rows: Vec::new(),
            reason: o.reason(),
            page: Page::LongHold,
        });
    }
    if cfg.corner.enabled && cfg.window.corner_resize {
        let affected = keys(&|r| r.key.starts_with("corner.") && r.key.ends_with(".diagonal"));
        if !affected.is_empty() {
            out.push(Conflict {
                kind: ConflictKind::Overlap,
                rows: affected,
                reason: "コーナースワイプが、ウィンドウのサイズ変更（角から反対側の角へなぞる）と重なります。"
                    .into(),
                page: Page::Corner,
            });
        }
    }
    for (thumb, page, set) in [
        (false, Page::FingerHold, &cfg.finger_hold),
        (true, Page::ThumbHold, &cfg.thumb_hold),
    ] {
        let mut repeated: Vec<String> = Vec::new();
        for (i, c) in set.combos.iter().enumerate() {
            let dup = set.combos[..i]
                .iter()
                .any(|d| d.hold == c.hold && d.trigger == c.trigger);
            if dup {
                let id = GestureId::hold_combo(thumb, c.hold, c.trigger);
                if !repeated.iter().any(|k| k == id.as_str()) {
                    repeated.push(id.as_str().to_owned());
                }
            }
        }
        if !repeated.is_empty() {
            out.push(Conflict {
                kind: ConflictKind::Overlap,
                rows: keys(&|r| repeated.contains(&r.key)),
                reason: "同じ指・同じ操作の組み合わせが複数あり、最初の1件だけが使われます。"
                    .into(),
                page,
            });
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------
// The page

const TAG_W: f32 = 168.0;
const GLOBAL_W: f32 = 120.0;
const PROFILE_W: f32 = 112.0;
const NAME_MIN_W: f32 = 140.0;

#[derive(Debug, Default)]
pub struct OverviewState {
    /// Columns (by lowercased exe name) the user hid.
    hidden: Vec<String>,
}

fn clipped(ui: &mut Ui, width: f32, text: RichText) {
    ui.allocate_ui_with_layout(
        Vec2::new(width, ui.spacing().interact_size.y),
        Layout::left_to_right(Align::Center),
        |ui| {
            ui.set_min_width(width);
            ui.set_max_width(width);
            let r = ui.add(Label::new(text.clone()).truncate());
            // Full text on hover when it was cut.
            r.on_hover_text(text.text());
        },
    );
}

pub fn page(
    ui: &mut Ui,
    t: &Tokens,
    cfg: &Config,
    win: &win_gestures::Values,
    state: &mut OverviewState,
) -> Option<Cmd> {
    let mut cmd = None;
    widgets::page_header(ui, t, Page::Overview.label(), Page::Overview.subtitle());
    let table = build(cfg, win);

    section_header(
        ui,
        t,
        &format!("確認が必要な項目（{} 件）", table.conflicts.len()),
    );
    group(ui, t, |g| {
        if table.conflicts.is_empty() {
            g.raw_row(|ui| {
                ui.label(
                    RichText::new("重なりや動かない割り当ては見つかりませんでした。")
                        .color(t.text_muted),
                );
            });
        }
        for c in &table.conflicts {
            g.wrapped_row(|ui| {
                tag(ui, t, c.kind.label());
                ui.label(RichText::new(&c.reason).size(t.fs_body));
                if widgets::button(ui, t, "開く").clicked() {
                    if let Some(tab) = c.rows.first().and_then(|k| super::features::tab_of_key(k)) {
                        super::features::request_gesture_tab(tab);
                    }
                    cmd = Some(Cmd::Goto(c.page));
                }
            });
        }
    });

    section_header(ui, t, "すべての割り当て");
    // Columns that fit; the user can hide some to see others.
    let width = ui.available_width();
    let room = width - 2.0 * t.row_pad_x - NAME_MIN_W - TAG_W - GLOBAL_W - 3.0 * t.space_md;
    let fit = ((room / (PROFILE_W + t.space_md)).floor().max(0.0) as usize).max(1);
    if table.columns.len() > 1 {
        group(ui, t, |g| {
            g.wrapped_row(|ui| {
                ui.label(RichText::new("表示するアプリ").color(t.text_muted));
                for c in &table.columns {
                    let shown = !state.hidden.contains(&c.key);
                    if ui.selectable_label(shown, &c.title).clicked() {
                        if shown {
                            state.hidden.push(c.key.clone());
                        } else {
                            state.hidden.retain(|k| *k != c.key);
                        }
                    }
                }
            });
        });
    }
    let visible: Vec<usize> = (0..table.columns.len())
        .filter(|i| !state.hidden.contains(&table.columns[*i].key))
        .collect();
    let shown: Vec<usize> = visible.iter().copied().take(fit).collect();
    if visible.len() > shown.len() {
        ui.label(
            RichText::new(format!(
                "幅が足りないため、アプリの列を {} 件省略しています。ウィンドウを広げるか、上で表示するアプリを減らしてください。",
                visible.len() - shown.len()
            ))
            .size(t.fs_small)
            .color(t.text_muted),
        );
        ui.add_space(t.space_xs);
    }

    group(ui, t, |g| {
        // Header.
        g.raw_row(|ui| {
            let name_w = (ui.available_width()
                - TAG_W
                - GLOBAL_W
                - shown.len() as f32 * PROFILE_W
                - (2.0 + shown.len() as f32) * ui.spacing().item_spacing.x)
                .max(NAME_MIN_W);
            let head = |s: &str| {
                RichText::new(s.to_owned())
                    .size(t.fs_small)
                    .color(t.text_muted)
            };
            clipped(ui, name_w, head("操作"));
            clipped(ui, TAG_W, head("注意"));
            clipped(ui, GLOBAL_W, head("全体"));
            for &i in &shown {
                let col = &table.columns[i];
                let r = ui
                    .allocate_ui_with_layout(
                        Vec2::new(PROFILE_W, ui.spacing().interact_size.y),
                        Layout::left_to_right(Align::Center),
                        |ui| {
                            ui.set_min_width(PROFILE_W);
                            ui.set_max_width(PROFILE_W);
                            ui.add(
                                Label::new(
                                    RichText::new(&col.title).size(t.fs_small).color(t.accent),
                                )
                                .truncate()
                                .sense(Sense::click()),
                            )
                            .on_hover_text(format!("{} の割り当てを編集する", col.title))
                        },
                    )
                    .inner;
                if r.clicked() {
                    cmd = Some(Cmd::OpenProfile(col.profile));
                }
            }
        });
        if table.rows.is_empty() {
            g.raw_row(|ui| {
                ui.label(RichText::new("割り当てはまだありません。").color(t.text_muted));
            });
        }
        for row in &table.rows {
            g.raw_row(|ui| {
                let origin = ui.cursor().min;
                let full = ui.available_width();
                let est = Rect::from_min_size(origin, Vec2::new(full, t.row_min_h_list));
                if ui.rect_contains_pointer(est) {
                    ui.painter().rect_filled(est, t.radius_sm, t.card_hover);
                }
                let name_w = (full
                    - TAG_W
                    - GLOBAL_W
                    - shown.len() as f32 * PROFILE_W
                    - (2.0 + shown.len() as f32) * ui.spacing().item_spacing.x)
                    .max(NAME_MIN_W);
                clipped(ui, name_w, RichText::new(&row.name).size(t.fs_body));
                let kinds = table.kinds_of(&row.key);
                ui.allocate_ui_with_layout(
                    Vec2::new(TAG_W, ui.spacing().interact_size.y),
                    Layout::left_to_right(Align::Center),
                    |ui| {
                        ui.set_min_width(TAG_W);
                        ui.set_max_width(TAG_W);
                        for k in &kinds {
                            tag(ui, t, k.label());
                        }
                    },
                );
                clipped(
                    ui,
                    GLOBAL_W,
                    RichText::new(&row.global)
                        .size(t.fs_body)
                        .color(t.text_muted),
                );
                for &i in &shown {
                    let (text, color) = match &row.cells[i] {
                        Cell::Inherit => ("—".to_owned(), t.text_muted),
                        Cell::Set(s) => (s.clone(), t.text),
                        Cell::Blocked => ("yubi を止める".to_owned(), t.text_muted),
                    };
                    clipped(
                        ui,
                        PROFILE_W,
                        RichText::new(text).size(t.fs_body).color(color),
                    );
                }
                let bottom = ui.min_rect().bottom().max(est.bottom());
                let hit = Rect::from_min_max(origin, Pos2::new(origin.x + full, bottom));
                let r = ui.interact(
                    hit,
                    ui.make_persistent_id(("overview-row", &row.key)),
                    Sense::click(),
                );
                if r.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                if r.on_hover_text(format!("「{}」を開く", row.page.label()))
                    .clicked()
                {
                    if let Some(tab) = super::features::tab_of_key(&row.key) {
                        super::features::request_gesture_tab(tab);
                    }
                    cmd = Some(Cmd::Goto(row.page));
                }
            });
        }
    });
    cmd
}

/// A small outlined text tag: the reason is carried by words, not only by color.
fn tag(ui: &mut Ui, t: &Tokens, text: &str) {
    egui::Frame::new()
        .fill(t.accent_soft)
        .stroke(Stroke::new(t.stroke_thin, t.accent))
        .corner_radius(t.radius_sm)
        .inner_margin(egui::Margin::symmetric(t.space_sm as i8, 1))
        .show(ui, |ui| {
            ui.label(RichText::new(text).size(t.fs_small).color(t.accent));
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Action, EdgeTarget};

    const WIN_OFF: win_gestures::Values = [Some(0); 4];
    const WIN_ON: win_gestures::Values = [None; 4];

    fn base() -> Config {
        let mut cfg = Config {
            gestures_enabled: true,
            ..Config::default()
        };
        cfg.drag.disable_all();
        cfg.ring.enabled = false;
        cfg.long_hold.enabled = false;
        cfg.long_hold.actions = crate::config::LongHoldActions::default();
        cfg.corner.enabled = false;
        cfg.four.enabled = true;
        cfg
    }

    fn profile(exe: &str, enabled: bool) -> Profile {
        Profile {
            exe_name: exe.to_owned(),
            enabled,
            ..Profile::default()
        }
    }

    fn row<'a>(t: &'a Table, key: &str) -> &'a Row {
        t.rows.iter().find(|r| r.key == key).expect(key)
    }

    #[test]
    fn names_are_japanese() {
        assert_eq!(gesture_name("tap.4"), "4本指でタップ");
        assert_eq!(gesture_name("swipe.4.left"), "4本指で左にスワイプ");
        assert_eq!(gesture_name("pinch.5.out"), "5本指でピンチアウト");
        assert_eq!(gesture_name("press.2.deep"), "2本指で深く押し込む");
        assert_eq!(
            gesture_name("corner.bl.diagonal"),
            "左下の角から斜めにスワイプ"
        );
        assert_eq!(gesture_name("edge.top"), "上端をなぞる");
        assert_eq!(
            gesture_name("hold.thumb.index.tap"),
            "サムホールド: 人差し指を置いてタップ"
        );
        assert_eq!(gesture_name("nope"), "nope");
    }

    #[test]
    fn every_gesture_id_has_a_japanese_name_and_a_page() {
        for id in GestureId::all() {
            let s = id.as_str();
            assert_ne!(gesture_name(s), s, "{s}");
            assert_ne!(page_of(s), Page::Home, "{s}");
        }
    }

    #[test]
    fn overridable_ids_skip_corner_hold_and_unbacked_ids() {
        let ids = overridable_ids(&Config::default());
        let has = |s: &str| ids.iter().any(|i| i.as_str() == s);
        assert!(has("swipe.4.left") && has("pinch.5.in") && has("press.3.deep"));
        assert!(has("tap.3") && has("swipe.3.left") && has("pinch.3.out"));
        assert!(
            !ids.iter()
                .any(|i| { i.as_str().starts_with("corner.") || i.as_str().starts_with("hold.") })
        );
    }

    #[test]
    fn rows_show_bound_gestures_and_skip_empty_ones() {
        let mut cfg = base();
        cfg.four.tap = Action::None;
        cfg.four.swipe_left = Action::Find;
        let t = build(&cfg, &WIN_OFF);
        assert_eq!(row(&t, "swipe.4.left").global, Action::Find.label());
        assert!(t.rows.iter().all(|r| r.key != "tap.4"));
        assert_eq!(row(&t, "swipe.4.left").page, Page::Gesture);
        assert!(t.columns.is_empty());
    }

    #[test]
    fn a_profile_that_overrides_gets_a_column_and_cells() {
        let mut cfg = base();
        cfg.four.tap = Action::None;
        cfg.four.swipe_left = Action::Find;
        let mut p = profile("code.exe", true);
        p.overrides.insert("swipe.4.left".into(), Action::None);
        p.overrides
            .insert("tap.4".into(), Action::Keys("Ctrl+W".into()));
        p.edge_overrides
            .insert("edge.left".into(), EdgeTarget::Volume);
        cfg.profiles.push(p);
        cfg.profiles.push(profile("quiet.exe", true)); // no overrides: no column
        let t = build(&cfg, &WIN_OFF);
        assert_eq!(t.columns.len(), 1);
        assert_eq!(t.columns[0].title, "code.exe");
        // Override of a gesture that is unbound globally still shows.
        assert_eq!(row(&t, "tap.4").cells[0], Cell::Set("Ctrl+W".into()));
        assert_eq!(row(&t, "swipe.4.left").cells[0], Cell::Set("なし".into()));
        assert_eq!(row(&t, "swipe.4.right").cells[0], Cell::Inherit);
        assert_eq!(row(&t, "edge.left").cells[0], Cell::Set("音量".into()));
    }

    #[test]
    fn a_blocked_profile_has_a_blocked_column() {
        let mut cfg = base();
        cfg.profiles.push(profile("game.exe", false));
        let t = build(&cfg, &WIN_OFF);
        assert_eq!(t.columns.len(), 1);
        assert!(t.columns[0].blocked);
        assert!(t.rows.iter().all(|r| r.cells[0] == Cell::Blocked));
    }

    #[test]
    fn the_same_action_on_several_gestures_is_not_a_conflict() {
        let mut cfg = base();
        cfg.four.swipe_left = Action::TaskView;
        cfg.four.swipe_right = Action::TaskView;
        cfg.four.swipe_up = Action::TaskView;
        let t = build(&cfg, &WIN_OFF);
        assert!(t.conflicts.is_empty(), "{:?}", t.conflicts);
    }

    #[test]
    fn disabled_feature_with_bindings_is_flagged_once_per_page() {
        let mut cfg = base();
        cfg.four.enabled = false;
        cfg.four.swipe_left = Action::Find;
        cfg.four.swipe_right = Action::Find;
        let t = build(&cfg, &WIN_OFF);
        let c: Vec<_> = t
            .conflicts
            .iter()
            .filter(|c| c.kind == ConflictKind::Disabled)
            .collect();
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].page, Page::Gesture);
        assert!(c[0].rows.contains(&"swipe.4.left".to_owned()));
        assert!(c[0].reason.contains("オフ"));
        assert_eq!(t.kinds_of("swipe.4.left"), vec![ConflictKind::Disabled]);
    }

    #[test]
    fn master_switch_off_flags_everything() {
        let mut cfg = base();
        cfg.gestures_enabled = false;
        cfg.four.swipe_left = Action::Find;
        let t = build(&cfg, &WIN_OFF);
        assert!(t.conflicts.iter().any(|c| c.page == Page::Home
            && c.kind == ConflictKind::Disabled
            && c.rows.contains(&"swipe.4.left".to_owned())));
    }

    #[test]
    fn windows_four_finger_gestures_win_until_disabled() {
        let mut cfg = base();
        cfg.four.swipe_left = Action::Find;
        let on = build(&cfg, &WIN_ON);
        let c = on
            .conflicts
            .iter()
            .find(|c| c.kind == ConflictKind::WindowsWins)
            .expect("windows conflict");
        assert_eq!(c.page, Page::General);
        assert!(c.rows.contains(&"swipe.4.left".to_owned()));
        assert!(
            build(&cfg, &WIN_OFF)
                .conflicts
                .iter()
                .all(|c| c.kind != ConflictKind::WindowsWins)
        );
    }

    #[test]
    fn three_finger_window_move_conflicts_with_windows() {
        let mut cfg = base();
        cfg.drag.three = crate::config::DragRow {
            enabled: true,
            start: crate::config::DragStart::TapThenMove,
            action: crate::config::DragAction::WindowMove,
        };
        let t = build(&cfg, &WIN_ON);
        assert!(
            t.conflicts
                .iter()
                .any(|c| c.kind == ConflictKind::WindowsWins
                    && c.rows.is_empty()
                    && c.reason.contains("3本指のドラッグ"))
        );
    }

    #[test]
    fn a_tap_then_move_row_overlaps_the_tap_of_its_count() {
        let mut cfg = base();
        cfg.drag.disable_all();
        cfg.drag.four = crate::config::DragRow {
            enabled: true,
            start: crate::config::DragStart::TapThenMove,
            action: crate::config::DragAction::WindowMove,
        };
        cfg.four.tap = Action::Find;
        let t = build(&cfg, &WIN_OFF);
        let c = t
            .conflicts
            .iter()
            .find(|c| c.kind == ConflictKind::Overlap && c.reason.contains("タップ後に動かす"))
            .expect("tap overlap");
        assert!(c.rows.contains(&"tap.4".to_owned()), "{c:?}");
        assert_eq!(c.page, Page::Drag);
        // The swipes of that count are not shadowed by a tap-then-move row.
        assert!(
            !t.conflicts
                .iter()
                .any(|c| c.reason.contains("スワイプはドラッグとして"))
        );
    }

    #[test]
    fn three_fingers_follow_the_rules_of_four_and_five() {
        assert_eq!(gesture_name("swipe.3.up"), "3本指で上にスワイプ");
        assert_eq!(page_of("pinch.3.in"), Page::Gesture);
        let mut cfg = base();
        cfg.three.swipe_left = Action::Find;
        // Off with a binding: flagged on its own, apart from the four-finger switch.
        let t = build(&cfg, &WIN_OFF);
        let c: Vec<_> = t
            .conflicts
            .iter()
            .filter(|c| c.kind == ConflictKind::Disabled)
            .collect();
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].rows, vec!["swipe.3.left".to_owned()]);
        assert!(c[0].reason.contains("3本指"));
        // On: no longer flagged, but Windows' three-finger gestures win while they are on.
        cfg.three.enabled = true;
        let t = build(&cfg, &WIN_OFF);
        assert!(t.conflicts.is_empty(), "{:?}", t.conflicts);
        let t = build(&cfg, &WIN_ON);
        let w = t
            .conflicts
            .iter()
            .find(|c| c.kind == ConflictKind::WindowsWins)
            .expect("windows wins");
        assert!(w.rows.contains(&"swipe.3.left".to_owned()));
        // A touch-and-move drag on the same count shadows the swipes.
        cfg.drag.only(3);
        let t = build(&cfg, &WIN_OFF);
        let o = t
            .conflicts
            .iter()
            .find(|c| c.kind == ConflictKind::Overlap)
            .expect("overlap");
        assert!(o.rows.contains(&"swipe.3.left".to_owned()));
    }

    #[test]
    fn drag_with_the_same_finger_count_overlaps_swipes() {
        let mut cfg = base();
        cfg.drag.only(4);
        cfg.four.swipe_left = Action::Find;
        let t = build(&cfg, &WIN_OFF);
        let c = t
            .conflicts
            .iter()
            .find(|c| c.kind == ConflictKind::Overlap)
            .expect("overlap");
        assert_eq!(c.page, Page::Drag);
        assert!(c.rows.contains(&"swipe.4.left".to_owned()));
        cfg.drag.only(3);
        assert!(
            build(&cfg, &WIN_OFF)
                .conflicts
                .iter()
                .all(|c| c.kind != ConflictKind::Overlap)
        );
    }

    #[test]
    fn drag_and_long_hold_with_the_same_count_overlap() {
        let mut cfg = base();
        cfg.drag.only(3);
        assert_eq!(drag_hold_overlap(&cfg), None);
        // The ring on 3 fingers needs the long hold and the ring on.
        cfg.long_hold.actions.three = Action::RingMenu;
        assert_eq!(drag_hold_overlap(&cfg), None);
        cfg.long_hold.enabled = true;
        cfg.ring.enabled = true;
        let o = drag_hold_overlap(&cfg).expect("ring overlap");
        assert_eq!((o.fingers, o.ring), (3, true));
        assert!(
            o.reason()
                .contains("3本指: ドラッグとロングホールド（リング）")
        );
        assert!(
            build(&cfg, &WIN_OFF)
                .conflicts
                .iter()
                .any(|c| c.kind == ConflictKind::Overlap
                    && c.reason.contains("リングが先に開きます"))
        );
        // Another count, or the drag off: no overlap.
        cfg.drag.only(4);
        assert_eq!(drag_hold_overlap(&cfg), None);
        cfg.drag.disable_all();
        assert_eq!(drag_hold_overlap(&cfg), None);
        // A plain action counts too; "none" does not.
        cfg.drag.only(3);
        cfg.long_hold.actions.three = Action::Find;
        let o = drag_hold_overlap(&cfg).expect("plain overlap");
        assert!(!o.ring);
        cfg.long_hold.actions.three = Action::None;
        assert_eq!(drag_hold_overlap(&cfg), None);
    }

    #[test]
    fn every_conflict_has_a_reason_and_a_place() {
        let mut cfg = base();
        cfg.four.enabled = false;
        cfg.four.swipe_left = Action::Find;
        cfg.drag.only(3);
        cfg.drag.four.enabled = true;
        for c in build(&cfg, &WIN_ON).conflicts {
            assert!(!c.reason.is_empty());
            assert_ne!(c.kind.label(), "");
        }
    }
}
