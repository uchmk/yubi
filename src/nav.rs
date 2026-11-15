//! Pages and the navigation tree.

use crate::config::Config;
use crate::icons;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Home,
    Drag,
    Four,
    Five,
    CustomPress,
    LongHold,
    FingerHold,
    ThumbHold,
    Corner,
    Edge,
    Window,
    Ring,
    Profiles,
    Haptic,
    Monitor,
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
            Page::Four,
            Page::Five,
            Page::CustomPress,
            Page::LongHold,
            Page::FingerHold,
            Page::ThumbHold,
            Page::Corner,
            Page::Edge,
            Page::Window,
        ],
    },
    Node::Item(Page::Ring),
    Node::Item(Page::Profiles),
    Node::Group {
        label: "デバイス",
        pages: &[Page::Haptic, Page::Monitor, Page::Diagnostics],
    },
    Node::Item(Page::General),
];

/// The feature pages with an enable toggle, in the order of `Config::feature_flags`.
pub const FEATURES: [Page; 11] = [
    Page::Drag,
    Page::Four,
    Page::Five,
    Page::CustomPress,
    Page::LongHold,
    Page::FingerHold,
    Page::ThumbHold,
    Page::Corner,
    Page::Edge,
    Page::Window,
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
            Self::Four => "4本指ジェスチャー",
            Self::Five => "5本指ジェスチャー",
            Self::CustomPress => "カスタムプレス",
            Self::LongHold => "ロングホールド",
            Self::FingerHold => "フィンガーホールド",
            Self::ThumbHold => "サムホールド",
            Self::Corner => "コーナー操作",
            Self::Edge => "エッジスライダー",
            Self::Window => "ウィンドウ操作",
            Self::Ring => "リングメニュー",
            Self::Profiles => "アプリ設定",
            Self::Haptic => "押し込みと触覚",
            Self::Monitor => "入力モニター",
            Self::Diagnostics => "診断",
            Self::General => "全般",
        }
    }

    /// One-line description: page subtitle and the muted line of the home list.
    pub fn subtitle(self) -> &'static str {
        match self {
            Self::Home => "Magic Trackpad の状態と設定を確認します。",
            Self::Drag => "複数本指でウィンドウや項目をドラッグします。",
            Self::Four => "4本指のタップ・スワイプ・ピンチに操作を割り当てます。",
            Self::Five => "5本指のピンチとスワイプに操作を割り当てます。",
            Self::CustomPress => "指の本数と押し込みの深さごとに操作を割り当てます。",
            Self::LongHold => "指を置き続けたときの操作を設定します。",
            Self::FingerHold => "ある指を置いたまま別の指で操作し、キーを送ります。",
            Self::ThumbHold => "親指を置いたまま別の指で操作し、キーを送ります。",
            Self::Corner => "トラックパッドの四隅に操作を割り当てます。",
            Self::Edge => "端をなぞって音量や明るさを調整します。",
            Self::Window => "ウィンドウの移動・スナップ・サイズ変更を設定します。",
            Self::Ring => "指を置き続けると現れるリング状のメニューです。",
            Self::Profiles => "アプリごとにジェスチャーの有効・無効を切り替えます。",
            Self::Haptic => "押し込みの深さのしきい値と、触覚フィードバックの強さです。",
            Self::Monitor => "トラックパッド上の接触をリアルタイムで表示します。",
            Self::Diagnostics => "デバイス情報、値の範囲、レポートレートを確認します。",
            Self::General => "テーマ、起動、設定フォルダー、バージョン情報です。",
        }
    }

    pub fn icon(self) -> char {
        match self {
            Self::Home => icons::HOME,
            Self::Drag => icons::DRAG,
            Self::Four => icons::FOUR_FINGER,
            Self::Five => icons::FIVE_FINGER,
            Self::CustomPress => icons::PRESS,
            Self::LongHold => icons::LONG_HOLD,
            Self::FingerHold => icons::FINGER_HOLD,
            Self::ThumbHold => icons::THUMB_HOLD,
            Self::Corner => icons::CORNER,
            Self::Edge => icons::EDGE,
            Self::Window => icons::WINDOW,
            Self::Ring => icons::RING,
            Self::Profiles => icons::APPS,
            Self::Haptic => icons::HAPTIC,
            Self::Monitor => icons::MONITOR,
            Self::Diagnostics => icons::DIAGNOSTICS,
            Self::General => icons::SETTINGS,
        }
    }

    /// The enable flag of a feature page.
    pub fn enabled_mut(self, cfg: &mut Config) -> Option<&mut bool> {
        Some(match self {
            Self::Drag => &mut cfg.drag.enabled,
            Self::Four => &mut cfg.four.enabled,
            Self::Five => &mut cfg.five.enabled,
            Self::CustomPress => &mut cfg.press.enabled,
            Self::LongHold => &mut cfg.long_hold.enabled,
            Self::FingerHold => &mut cfg.finger_hold.enabled,
            Self::ThumbHold => &mut cfg.thumb_hold.enabled,
            Self::Corner => &mut cfg.corner.enabled,
            Self::Edge => &mut cfg.edge.enabled,
            Self::Window => &mut cfg.window.enabled,
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
        for (i, page) in FEATURES.iter().enumerate() {
            let before = cfg.feature_flags()[i];
            *page.enabled_mut(&mut cfg).unwrap() = !before;
            assert_eq!(cfg.feature_flags()[i], !before, "{page:?}");
        }
    }

    #[test]
    fn every_page_is_in_the_nav() {
        let pages = all_pages();
        assert_eq!(pages.len(), 17);
        assert!(pages.contains(&Page::Home) && pages.contains(&Page::General));
    }
}
