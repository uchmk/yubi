//! What a ring slot looks like: the icon glyph of a built-in action (Segoe Fluent Icons / Segoe
//! MDL2 Assets code points), or the app's own picture for a "launch an app" item.

use crate::config::Action;
use crate::overlay::RingImage;

/// Generic keyboard (for a key combination).
pub const KEYBOARD: char = '\u{E765}';
const SEARCH: char = '\u{E721}';
const MOUSE: char = '\u{E962}';
const VOLUME_UP: char = '\u{E995}';
const VOLUME_DOWN: char = '\u{E993}';
const MUTE: char = '\u{E74F}';
const BRIGHTNESS: char = '\u{E706}';
const TASK_VIEW: char = '\u{E7C4}';
const DESKTOP: char = '\u{E7F4}';
const PREV: char = '\u{E72B}';
const NEXT: char = '\u{E72A}';
const APP_SWITCH: char = '\u{E8F9}';
const TAB_SWITCH: char = '\u{E8A1}';
const WINDOW_ARRANGE: char = '\u{E740}';
const WINDOW_ZOOM: char = '\u{E8A3}';
/// A plain application window, for a launch item whose own icon is not (yet) known.
const APP: char = '\u{ECAA}';

/// The glyph a built-in action gets, if any.
pub fn glyph_of(action: &Action) -> Option<char> {
    Some(match action {
        Action::TaskView => TASK_VIEW,
        Action::ShowDesktop => DESKTOP,
        Action::DesktopPrev => PREV,
        Action::DesktopNext => NEXT,
        Action::AppSwitch => APP_SWITCH,
        Action::TabSwitch => TAB_SWITCH,
        Action::WindowArrange(_) => WINDOW_ARRANGE,
        Action::WindowZoom => WINDOW_ZOOM,
        Action::Find => SEARCH,
        Action::MiddleClick | Action::RightClick => MOUSE,
        Action::Keys(_) => KEYBOARD,
        Action::VolumeStep(n) if *n < 0 => VOLUME_DOWN,
        Action::VolumeStep(_) => VOLUME_UP,
        Action::BrightnessStep(_) => BRIGHTNESS,
        Action::ToggleMute => MUTE,
        Action::Launch { .. } => APP,
        Action::RingMenu | Action::ResizeFromOpposite | Action::None => return None,
    })
}

/// How one slot is drawn.
#[cfg_attr(
    test,
    allow(
        dead_code,
        reason = "only the overlay path, which tests stub out, uses it"
    )
)]
pub struct Look {
    pub glyph: Option<char>,
    pub image: Option<RingImage>,
}

/// The glyph, and for a launch item the app's own picture (from the cache; the shell is asked
/// the first time).
#[cfg_attr(
    test,
    allow(
        dead_code,
        reason = "only the overlay path, which tests stub out, uses it"
    )
)]
pub fn look_of(action: &Action) -> Look {
    let image = match action {
        Action::Launch { path, .. } if !path.trim().is_empty() => {
            crate::os::appicon::icon_for(path).map(|i| RingImage {
                side: i.side,
                px: i.px,
            })
        }
        _ => None,
    };
    Look {
        glyph: glyph_of(action),
        image,
    }
}

/// True when `label` is just the action's own name (the user typed no label of their own).
pub fn is_default_label(action: &Action, label: &str) -> bool {
    label == action.label()
}

/// The label to show: a launch item without a label of its own shows the app's file
/// description (or, failing that, the file name).
pub fn slot_label(action: &Action, label: &str) -> String {
    match action {
        Action::Launch { path, .. }
            if !path.trim().is_empty() && is_default_label(action, label) =>
        {
            crate::os::appicon::label_for(path)
        }
        _ => label.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ActionKind;

    #[test]
    fn built_in_actions_have_glyphs() {
        for kind in ActionKind::ALL {
            let mut a = Action::None;
            a.set_kind(kind);
            let glyph = glyph_of(&a);
            match kind {
                ActionKind::None | ActionKind::RingMenu => assert_eq!(glyph, None, "{kind:?}"),
                _ => assert!(glyph.is_some(), "{kind:?} has no glyph"),
            }
        }
    }

    #[test]
    fn keys_get_a_keyboard_and_volume_a_direction() {
        assert_eq!(glyph_of(&Action::Keys("Ctrl+C".into())), Some(KEYBOARD));
        assert_ne!(
            glyph_of(&Action::VolumeStep(5)),
            glyph_of(&Action::VolumeStep(-5))
        );
        // All glyphs are in the private-use range of the icon fonts.
        for a in [
            Action::TaskView,
            Action::Find,
            Action::ToggleMute,
            Action::VolumeStep(1),
        ] {
            let c = glyph_of(&a).unwrap();
            assert!(('\u{E000}'..='\u{F8FF}').contains(&c), "{a:?}");
        }
    }

    #[test]
    fn the_default_label_is_recognised() {
        let a = Action::Launch {
            path: r"C:\x\Code.exe".into(),
            args: String::new(),
        };
        assert!(is_default_label(&a, "Code"));
        assert!(!is_default_label(&a, "エディタ"));
        // A user label is kept as it is, and other actions do not look anything up.
        assert_eq!(slot_label(&a, "エディタ"), "エディタ");
        assert_eq!(slot_label(&Action::Find, "検索"), "検索");
    }

    /// Draws rings of 4, 6 and 8 items to BMP files when `YUBI_RINGSHOT=<dir>` is set (a
    /// development aid; otherwise it only checks that something is drawn).
    #[cfg(windows)]
    #[test]
    fn ring_shots() {
        use crate::overlay::{RingImage, RingSlot};
        use std::sync::Arc;
        let acts = [
            (Action::TaskView, "タスクビュー"),
            (Action::Keys("Ctrl+C".into()), "コピー"),
            (Action::VolumeStep(5), "音量を上げる"),
            (Action::Find, "検索"),
            (
                Action::Launch {
                    path: "x.exe".into(),
                    args: String::new(),
                },
                "Visual Studio Code",
            ),
            (Action::ShowDesktop, "デスクトップを表示"),
            (Action::ToggleMute, "ミュート"),
            (Action::DesktopNext, "次のデスクトップ"),
        ];
        let dir = std::env::var_os("YUBI_RINGSHOT").map(std::path::PathBuf::from);
        for (n, hl, centre, dpi) in [
            (4, Some(1), false, 96),
            (6, Some(2), false, 96),
            (8, Some(5), false, 96),
            (8, None, true, 96),
            (6, Some(0), false, 144),
        ] {
            let items: Vec<RingSlot> = acts[..n]
                .iter()
                .map(|(a, l)| {
                    // A made-up app picture for the launch item.
                    let image = matches!(a, Action::Launch { .. }).then(|| RingImage {
                        side: 4,
                        px: Arc::new(vec![0xFF2E7DD6; 16]),
                    });
                    RingSlot {
                        label: (*l).into(),
                        icon: glyph_of(a),
                        image,
                    }
                })
                .collect();
            let (side, px) = crate::overlay::snapshot(items, hl, centre, dpi).unwrap();
            assert!(px.iter().any(|&p| p >> 24 != 0), "n={n}: nothing drawn");
            if let Some(dir) = &dir {
                for (bg_name, bg) in [("dark", 0x30u32), ("light", 0xD8)] {
                    let mut out = Vec::new();
                    let (w, h) = (side as u32, side as u32);
                    out.extend_from_slice(b"BM");
                    out.extend_from_slice(&(54 + w * h * 4).to_le_bytes());
                    out.extend_from_slice(&[0; 4]);
                    out.extend_from_slice(&54u32.to_le_bytes());
                    out.extend_from_slice(&40u32.to_le_bytes());
                    out.extend_from_slice(&w.to_le_bytes());
                    out.extend_from_slice(&(-(h as i32)).to_le_bytes());
                    out.extend_from_slice(&1u16.to_le_bytes());
                    out.extend_from_slice(&32u16.to_le_bytes());
                    out.extend_from_slice(&[0; 24]);
                    for p in &px {
                        let a = p >> 24;
                        let c = |sh: u32| (((p >> sh) & 0xFF) + bg * (255 - a) / 255).min(255);
                        let q = c(0) | c(8) << 8 | c(16) << 16 | 0xFF << 24;
                        out.extend_from_slice(&q.to_le_bytes());
                    }
                    let name = format!(
                        "overlay-{n}{}-{dpi}-{bg_name}.bmp",
                        if centre { "-centre" } else { "" }
                    );
                    std::fs::write(dir.join(name), out).unwrap();
                }
            }
        }
    }
}
