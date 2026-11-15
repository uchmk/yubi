//! Pages and the navigation tree.

use crate::config::Config;
#[cfg(test)]
use crate::config::FEATURE_COUNT;
use crate::icons;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Home,
    Drag,
    Gesture,
    CustomPress,
    LongHold,
    FingerHold,
    ThumbHold,
    Corner,
    Edge,
    Ring,
    Profiles,
    Overview,
    Sensitivity,
    Haptic,
    Practice,
    Diagnostics,
    General,
}

/// One entry of the navigation pane.
pub enum Node {
    Item(Page),
    Group {
        label: &'static str,
        pages: &'static [Page],
    },
}

pub const NAV: [Node; 6] = [
    Node::Item(Page::Home),
    Node::Group {
        label: "操作の割り当て",
        pages: &[
            Page::Drag,
            Page::Gesture,
            Page::CustomPress,
            Page::LongHold,
            Page::FingerHold,
            Page::ThumbHold,
            Page::Corner,
            Page::Edge,
            Page::Ring,
        ],
    },
    Node::Item(Page::Overview),
    Node::Item(Page::Profiles),
    Node::Group {
        label: "デバイス",
        pages: &[
            Page::Sensitivity,
            Page::Haptic,
            Page::Practice,
            Page::Diagnostics,
        ],
    },
    Node::Item(Page::General),
];

/// The feature pages with an enable toggle (the home list). `Gesture` carries two flags (4 and 5
/// fingers); the 4-finger flag is the one `enabled_mut` returns, the page edits both.
pub const FEATURES: [Page; 9] = [
    Page::Drag,
    Page::Gesture,
    Page::CustomPress,
    Page::LongHold,
    Page::FingerHold,
    Page::ThumbHold,
    Page::Corner,
    Page::Edge,
    Page::Ring,
];

/// Every page in navigation order.
pub fn all_pages() -> Vec<Page> {
    let mut pages = Vec::new();
    for node in &NAV {
        match node {
            Node::Item(p) => pages.push(*p),
            Node::Group { pages: ps, .. } => pages.extend(ps.iter().copied()),
        }
    }
    pages
}

impl Page {
    pub fn label(self) -> &'static str {
        match self {
            Self::Home => "ホーム",
            Self::Drag => "ドラッグ",
            Self::Gesture => "ジェスチャー",
            Self::CustomPress => "カスタムプレス",
            Self::LongHold => "ロングホールド",
            Self::FingerHold => "フィンガーホールド",
            Self::ThumbHold => "サムホールド",
            Self::Corner => "コーナー操作",
            Self::Edge => "エッジスライダー",
            Self::Ring => "リングメニュー",
            Self::Profiles => "アプリごとの割り当て",
            Self::Overview => "割り当ての一覧",
            Self::Sensitivity => "感度",
            Self::Haptic => "押し込みと触覚",
            Self::Practice => "入力の確認",
            Self::Diagnostics => "診断",
            Self::General => "全般",
        }
    }

    /// One-line description: page subtitle and the muted line of the home list.
    pub fn subtitle(self) -> &'static str {
        match self {
            Self::Home => "Magic Trackpad の状態と設定を確認します。",
            Self::Drag => "ドラッグの始め方と、保持・一時停止を設定します。",
            Self::Gesture => "4本指・5本指のタップ・スワイプ・ピンチに操作を割り当てます。",
            Self::CustomPress => "指の本数と押し込みの深さごとに操作を割り当てます。",
            Self::LongHold => "指を置き続けたときの操作を設定します。",
            Self::FingerHold => "ある指を置いたまま別の指で操作し、キーを送ります。",
            Self::ThumbHold => "親指を置いたまま別の指で操作し、キーを送ります。",
            Self::Corner => "トラックパッドの四隅に操作を割り当てます。",
            Self::Edge => "端をなぞって音量や明るさを調整します。",
            Self::Ring => "指を置き続けると現れるリング状のメニューです。",
            Self::Profiles => {
                "アプリごとに、ジェスチャーの操作を差し替えたり、yubi を止めたりします。"
            }
            Self::Overview => "すべての割り当てと、重なって動かない設定を一度に確認します。",
            Self::Sensitivity => "ドラッグの速さ、タップやスワイプの判定の厳しさを調整します。",
            Self::Haptic => {
                "押し込みの強さをリアルタイムで確かめ、しきい値の目安を測ります。触覚フィードバックの設定もあります。"
            }
            Self::Practice => {
                "トラックパッド上の接触をリアルタイムで表示し、認識されたジェスチャーと操作を実行せずに確かめます。"
            }
            Self::Diagnostics => "デバイス情報、値の範囲、レポートレートを確認します。",
            Self::General => "テーマ、起動、設定フォルダー、バージョン情報です。",
        }
    }

    pub fn icon(self) -> char {
        match self {
            Self::Home => icons::HOME,
            Self::Drag => icons::DRAG,
            Self::Gesture => icons::FOUR_FINGER,
            Self::CustomPress => icons::PRESS,
            Self::LongHold => icons::LONG_HOLD,
            Self::FingerHold => icons::FINGER_HOLD,
            Self::ThumbHold => icons::THUMB_HOLD,
            Self::Corner => icons::CORNER,
            Self::Edge => icons::EDGE,
            Self::Ring => icons::RING,
            Self::Profiles => icons::APPS,
            Self::Overview => icons::LOG,
            Self::Sensitivity => icons::SENSITIVITY,
            Self::Haptic => icons::HAPTIC,
            Self::Practice => icons::MONITOR,
            Self::Diagnostics => icons::DIAGNOSTICS,
            Self::General => icons::SETTINGS,
        }
    }

    /// Whether the page lays itself out in two columns when the window is wide (see
    /// `widgets::sections2`), so it may use more than the usual content width.
    pub fn wide(self) -> bool {
        matches!(
            self,
            Self::Drag | Self::CustomPress | Self::Corner | Self::Sensitivity | Self::General
        )
    }

    /// Whether the feature page's switch reads as on. The drag page has one switch per finger
    /// count, so it reads as on when any row is.
    pub fn is_on(self, cfg: &mut Config) -> bool {
        match self {
            Self::Drag => cfg.drag.any(),
            _ => self.enabled_mut(cfg).is_some_and(|f| *f),
        }
    }

    /// Set the feature page's switch. For the drag page, on turns the three-finger row on when no
    /// row is, and off turns every row off.
    pub fn set_on(self, cfg: &mut Config, on: bool) {
        match self {
            Self::Drag if on => {
                if !cfg.drag.any() {
                    cfg.drag.three.enabled = true;
                }
            }
            Self::Drag => cfg.drag.disable_all(),
            _ => {
                if let Some(f) = self.enabled_mut(cfg) {
                    *f = on;
                }
            }
        }
    }

    /// The enable flag of a feature page (not the drag page: see [`Self::set_on`]).
    pub fn enabled_mut(self, cfg: &mut Config) -> Option<&mut bool> {
        Some(match self {
            Self::Gesture => &mut cfg.four.enabled,
            Self::CustomPress => &mut cfg.press.enabled,
            Self::LongHold => &mut cfg.long_hold.enabled,
            Self::FingerHold => &mut cfg.finger_hold.enabled,
            Self::ThumbHold => &mut cfg.thumb_hold.enabled,
            Self::Corner => &mut cfg.corner.enabled,
            Self::Edge => &mut cfg.edge.enabled,
            Self::Ring => &mut cfg.ring.enabled,
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn features_match_config_flags() {
        let mut cfg = Config::default();
        let mut covered = vec![false; FEATURE_COUNT];
        for page in FEATURES {
            let before = cfg.feature_flags();
            let on = page.is_on(&mut cfg);
            page.set_on(&mut cfg, !on);
            let after = cfg.feature_flags();
            let changed: Vec<usize> = (0..FEATURE_COUNT)
                .filter(|&i| before[i] != after[i])
                .collect();
            assert_eq!(changed.len(), 1, "{page:?}");
            covered[changed[0]] = true;
        }
        // The ジェスチャー page has a second switch (5 fingers) that `enabled_mut` does not return.
        let before = cfg.feature_flags();
        cfg.five.enabled ^= true;
        let after = cfg.feature_flags();
        for i in 0..FEATURE_COUNT {
            covered[i] |= before[i] != after[i];
        }
        assert!(
            covered.iter().all(|c| *c),
            "a flag has no page: {covered:?}"
        );
    }

    #[test]
    fn every_page_is_in_the_nav() {
        let pages = all_pages();
        assert_eq!(pages.len(), 17);
        assert!(pages.contains(&Page::Home) && pages.contains(&Page::General));
    }
}
