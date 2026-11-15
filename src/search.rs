//! Settings search: a pure filter over a static index of pages and rows.

use crate::nav::{Page, all_pages};

pub struct Entry {
    pub page: Page,
    pub title: &'static str,
    pub desc: &'static str,
}

const fn e(page: Page, title: &'static str, desc: &'static str) -> Entry {
    Entry { page, title, desc }
}

/// Searchable rows inside pages. The pages themselves are added by `index`.
const ROWS: &[Entry] = &[
    e(Page::Drag, "指の本数", "ドラッグに使う指の本数"),
    e(
        Page::Drag,
        "指を離してから終了するまでの時間",
        "ドラッグが終わるまでの猶予",
    ),
    e(
        Page::Drag,
        "離したあともドラッグを続ける",
        "指を離しても押したままにする",
    ),
    e(Page::Four, "タップ", "4本指で軽く触れる"),
    e(Page::Four, "左にスワイプ", "4本指で左へなぞる"),
    e(Page::Four, "右にスワイプ", "4本指で右へなぞる"),
    e(Page::Four, "上にスワイプ", "4本指で上へなぞる"),
    e(Page::Four, "下にスワイプ", "4本指で下へなぞる"),
    e(Page::Four, "ピンチイン", "4本指で指を寄せる"),
    e(Page::Four, "ピンチアウト", "4本指で指を広げる"),
    e(Page::Five, "ピンチイン", "5本指で指を寄せる"),
    e(Page::Five, "ピンチアウト", "5本指で指を広げる"),
    e(Page::Five, "上にスワイプ", "5本指で上へなぞる"),
    e(Page::Five, "下にスワイプ", "5本指で下へなぞる"),
    e(
        Page::CustomPress,
        "軽い押し込みのしきい値",
        "押し込みの浅さ",
    ),
    e(
        Page::CustomPress,
        "深い押し込みのしきい値",
        "押し込みの深さ",
    ),
    e(
        Page::CustomPress,
        "指の本数ごとの割り当て",
        "1本指から5本指まで",
    ),
    e(Page::LongHold, "指の本数", "長押しに使う指の本数"),
    e(Page::LongHold, "長押しの時間", "操作が始まるまでの時間"),
    e(Page::LongHold, "実行する操作", "長押しで実行"),
    e(
        Page::FingerHold,
        "キーの組み合わせ",
        "指を置いたままタップやスワイプでキーを送る",
    ),
    e(
        Page::ThumbHold,
        "キーの組み合わせ",
        "親指を置いたままタップやスワイプでキーを送る",
    ),
    e(Page::Corner, "左上", "左上の角の押し込みとスワイプ"),
    e(Page::Corner, "右上", "右上の角の押し込みとスワイプ"),
    e(Page::Corner, "左下", "左下の角の押し込みとスワイプ"),
    e(Page::Corner, "右下", "右下の角の押し込みとスワイプ"),
    e(Page::Edge, "左端", "左端をなぞって調整"),
    e(Page::Edge, "右端", "右端をなぞって調整"),
    e(Page::Edge, "上端", "上端をなぞって調整"),
    e(
        Page::Window,
        "3本指でウィンドウを移動",
        "タップしてからスワイプ",
    ),
    e(
        Page::Window,
        "フリックでスナップ",
        "ウィンドウを画面の端に寄せる",
    ),
    e(
        Page::Window,
        "コーナースワイプでサイズ変更",
        "反対側の角でウィンドウサイズを変更",
    ),
    e(Page::Ring, "指の本数", "リングメニューを開く指の本数"),
    e(
        Page::Ring,
        "長押しの時間",
        "リングメニューが現れるまでの時間",
    ),
    e(Page::Ring, "メニュー項目", "表示名と操作"),
    e(
        Page::Profiles,
        "アプリを追加",
        "実行中のアプリや実行ファイル名から登録",
    ),
    e(
        Page::Profiles,
        "このアプリでは yubi を止める",
        "アプリごとにジェスチャーを無効にする",
    ),
    e(
        Page::Profiles,
        "全体と同じ",
        "アプリごとに操作を差し替える（上書き）",
    ),
    e(
        Page::Overview,
        "確認が必要な項目",
        "重なって区別できない操作、動かない割り当て",
    ),
    e(
        Page::Overview,
        "すべての割り当て",
        "ジェスチャーごとの全体の操作とアプリごとの上書き",
    ),
    e(
        Page::Sensitivity,
        "ドラッグの速さ",
        "指の動きに対するドラッグの速さ",
    ),
    e(
        Page::Sensitivity,
        "ウィンドウ移動の速さ",
        "3本指ウィンドウ移動の速さ",
    ),
    e(
        Page::Sensitivity,
        "スワイプの距離",
        "スワイプと判定する指の移動量",
    ),
    e(
        Page::Sensitivity,
        "ドラッグ開始の距離",
        "ドラッグが始まるまでの距離",
    ),
    e(
        Page::Sensitivity,
        "ウィンドウ移動の開始距離",
        "ウィンドウ移動が始まるまでの距離",
    ),
    e(
        Page::Sensitivity,
        "タップの長さ",
        "タップと判定する最長の時間",
    ),
    e(
        Page::Sensitivity,
        "タップの許容移動",
        "タップ中に動いてよい距離",
    ),
    e(
        Page::Sensitivity,
        "長押しの時間",
        "長押しと判定するまでの時間",
    ),
    e(
        Page::Sensitivity,
        "長押しの許容移動",
        "長押し中に動いてよい距離",
    ),
    e(
        Page::Sensitivity,
        "ウィンドウ移動までの間隔",
        "タップからスワイプまでの猶予",
    ),
    e(
        Page::Sensitivity,
        "ピンチインの比率",
        "ピンチインと判定する縮小率",
    ),
    e(
        Page::Sensitivity,
        "ピンチアウトの比率",
        "ピンチアウトと判定する拡大率",
    ),
    e(
        Page::Sensitivity,
        "フリックの速さ",
        "フリックと判定する速さ",
    ),
    e(Page::Sensitivity, "端の幅", "エッジスライダーの反応する幅"),
    e(
        Page::Sensitivity,
        "端の刻み",
        "エッジスライダーの1ステップの距離",
    ),
    e(
        Page::Sensitivity,
        "カーソルの速度",
        "タッチパッドのカーソルの速さ。アプリごとにも切り替え",
    ),
    e(
        Page::Profiles,
        "カーソルの速度（アプリごと）",
        "このアプリだけタッチパッドのカーソルの速さを変える",
    ),
    e(
        Page::Sensitivity,
        "yubi でカーソルの速度を管理する",
        "Windows のタッチパッドの速度を yubi が保つ",
    ),
    e(Page::Sensitivity, "既定に戻す", "感度の値を初期値にします"),
    e(Page::Haptic, "押し込みの強さ", "ライブ表示と測定"),
    e(
        Page::Haptic,
        "キャリブレーション",
        "押し込みの強さを測る (任意)",
    ),
    e(
        Page::CustomPress,
        "軽い押し込みのしきい値",
        "0.01 から 0.20",
    ),
    e(
        Page::CustomPress,
        "深い押し込みのしきい値",
        "0.01 から 0.20",
    ),
    e(Page::Haptic, "触覚の強さ", "触覚フィードバック"),
    e(
        Page::Practice,
        "練習モード",
        "ジェスチャーを実行せずに確かめる",
    ),
    e(Page::Practice, "最近の判定", "認識されたジェスチャーの履歴"),
    e(Page::Diagnostics, "レポートレート", "1秒あたりのフレーム数"),
    e(Page::General, "テーマ", "黒・システムに合わせる・ライト"),
    e(
        Page::General,
        "Windows の起動時に開始",
        "ログイン時に自動で起動",
    ),
    e(Page::General, "設定フォルダー", "設定ファイルの場所"),
    e(Page::General, "バージョン", "yubi のバージョン"),
];

/// All pages (label + subtitle) followed by the rows.
pub fn index() -> Vec<Entry> {
    all_pages()
        .into_iter()
        .map(|p| e(p, p.label(), p.subtitle()))
        .chain(ROWS.iter().map(|r| e(r.page, r.title, r.desc)))
        .collect()
}

/// Entries matching every whitespace-separated term of `query` in title or description
/// (case-insensitive). Title matches rank before description-only matches; ties keep index order.
pub fn filter<'a>(entries: &'a [Entry], query: &str) -> Vec<&'a Entry> {
    let terms: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    if terms.is_empty() {
        return Vec::new();
    }
    let mut hits: Vec<(bool, &Entry)> = entries
        .iter()
        .filter_map(|en| {
            let title = en.title.to_lowercase();
            let desc = en.desc.to_lowercase();
            let all = terms
                .iter()
                .all(|t| title.contains(t.as_str()) || desc.contains(t.as_str()));
            all.then(|| (terms.iter().all(|t| title.contains(t.as_str())), en))
        })
        .collect();
    // Stable sort: title matches first, index order otherwise.
    hits.sort_by_key(|(in_title, _)| !*in_title);
    hits.into_iter().map(|(_, en)| en).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_query_matches_nothing() {
        let idx = index();
        assert!(filter(&idx, "").is_empty());
        assert!(filter(&idx, "   ").is_empty());
    }

    #[test]
    fn title_matches_rank_first() {
        let idx = index();
        let hits = filter(&idx, "触覚");
        assert!(!hits.is_empty());
        assert!(hits[0].title.contains("触覚"));
        assert!(hits.iter().any(|h| h.page == Page::Haptic));
    }

    #[test]
    fn terms_must_all_match() {
        let idx = index();
        let hits = filter(&idx, "テーマ 黒");
        assert!(!hits.is_empty());
        assert!(hits.iter().all(|h| h.page == Page::General));
        assert!(filter(&idx, "テーマ zzzz").is_empty());
    }

    #[test]
    fn description_and_case_insensitive() {
        let idx = index();
        let hits = filter(&idx, "YUBI");
        assert!(hits.iter().any(|h| h.title == "バージョン"));
    }

    #[test]
    fn every_page_is_searchable_by_label() {
        let idx = index();
        for p in all_pages() {
            assert!(filter(&idx, p.label()).iter().any(|h| h.page == p));
        }
    }
}
