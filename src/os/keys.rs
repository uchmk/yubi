//! Key-combo strings ("Ctrl+Shift+T", "Win+Left", "VolumeUp") <-> `KeyCombo`,
//! and conversion to `SendInput` structures.

use std::fmt;
use std::str::FromStr;

use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY,
    KEYEVENTF_KEYUP, MAPVK_VK_TO_VSC, MapVirtualKeyW, SendInput, VIRTUAL_KEY,
};

use super::YUBI_INJECT_MARK;

/// A Windows virtual-key code.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Vk(pub u16);

pub const VK_SHIFT: Vk = Vk(0x10);
pub const VK_CONTROL: Vk = Vk(0x11);
pub const VK_MENU: Vk = Vk(0x12); // Alt
pub const VK_LWIN: Vk = Vk(0x5B);

/// Canonical modifier order used for `Display` and normalisation.
const MODIFIER_ORDER: [(Vk, &str); 4] = [
    (VK_CONTROL, "Ctrl"),
    (VK_MENU, "Alt"),
    (VK_SHIFT, "Shift"),
    (VK_LWIN, "Win"),
];

/// Named (non letter/digit/F-key) keys. The name here is the canonical one.
const NAMED_KEYS: &[(&str, u16)] = &[
    ("Tab", 0x09),
    ("Enter", 0x0D),
    ("Esc", 0x1B),
    ("Space", 0x20),
    ("Backspace", 0x08),
    ("Delete", 0x2E),
    ("Insert", 0x2D),
    ("Home", 0x24),
    ("End", 0x23),
    ("PageUp", 0x21),
    ("PageDown", 0x22),
    ("Left", 0x25),
    ("Up", 0x26),
    ("Right", 0x27),
    ("Down", 0x28),
    ("PrintScreen", 0x2C),
    ("Pause", 0x13),
    ("CapsLock", 0x14),
    ("Apps", 0x5D),
    ("VolumeMute", 0xAD),
    ("VolumeDown", 0xAE),
    ("VolumeUp", 0xAF),
    ("MediaNext", 0xB0),
    ("MediaPrev", 0xB1),
    ("MediaStop", 0xB2),
    ("MediaPlayPause", 0xB3),
    ("BrowserBack", 0xA6),
    ("BrowserForward", 0xA7),
    ("BrowserRefresh", 0xA8),
    ("BrowserHome", 0xAC),
    ("Semicolon", 0xBA),
    ("Plus", 0xBB),
    ("Comma", 0xBC),
    ("Minus", 0xBD),
    ("Period", 0xBE),
    ("Slash", 0xBF),
    ("Backquote", 0xC0),
    ("LeftBracket", 0xDB),
    ("Backslash", 0xDC),
    ("RightBracket", 0xDD),
    ("Quote", 0xDE),
];

/// Aliases accepted on input (matched lowercase), mapped to canonical names.
const ALIASES: &[(&str, &str)] = &[
    ("control", "Ctrl"),
    ("ctl", "Ctrl"),
    ("option", "Alt"),
    ("menu", "Alt"),
    ("windows", "Win"),
    ("super", "Win"),
    ("meta", "Win"),
    ("cmd", "Win"),
    ("escape", "Esc"),
    ("return", "Enter"),
    ("bksp", "Backspace"),
    ("del", "Delete"),
    ("ins", "Insert"),
    ("pgup", "PageUp"),
    ("pgdn", "PageDown"),
    ("pagedn", "PageDown"),
    ("prtsc", "PrintScreen"),
    ("playpause", "MediaPlayPause"),
    ("nexttrack", "MediaNext"),
    ("prevtrack", "MediaPrev"),
    ("mute", "VolumeMute"),
];

/// Why a key-combo string could not be parsed (messages are Japanese, for UI).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyParseError {
    Empty,
    UnknownKey(String),
    /// Only modifiers were given ("Ctrl+Shift").
    MissingKey,
    /// Two non-modifier keys ("A+B").
    MultipleKeys(String, String),
}

impl fmt::Display for KeyParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "キーが指定されていません"),
            Self::UnknownKey(s) => write!(f, "「{s}」は認識できないキーです"),
            Self::MissingKey => write!(f, "修飾キーのほかに、通常のキーを 1 つ指定してください"),
            Self::MultipleKeys(a, b) => {
                write!(f, "通常のキーは 1 つだけ指定できます(「{a}」と「{b}」)")
            }
        }
    }
}

impl std::error::Error for KeyParseError {}

/// A shortcut: zero or more modifiers held while `key` is tapped.
/// `modifiers` is kept de-duplicated in canonical order (Ctrl, Alt, Shift, Win).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct KeyCombo {
    pub modifiers: Vec<Vk>,
    pub key: Vk,
}

impl KeyCombo {
    pub fn new(modifiers: impl IntoIterator<Item = Vk>, key: Vk) -> Self {
        let given: Vec<Vk> = modifiers.into_iter().collect();
        let modifiers = MODIFIER_ORDER
            .iter()
            .map(|(v, _)| *v)
            .filter(|v| given.contains(v))
            .collect();
        Self { modifiers, key }
    }
}

fn modifier_of(name: &str) -> Option<Vk> {
    MODIFIER_ORDER
        .iter()
        .find(|(_, n)| n.eq_ignore_ascii_case(name))
        .map(|(v, _)| *v)
}

fn key_of(name: &str) -> Option<Vk> {
    let b = name.as_bytes();
    if b.len() == 1 {
        let c = b[0].to_ascii_uppercase();
        if c.is_ascii_uppercase() || c.is_ascii_digit() {
            return Some(Vk(c as u16));
        }
    }
    if (b.len() == 2 || b.len() == 3)
        && (b[0] == b'f' || b[0] == b'F')
        && let Ok(n) = name[1..].parse::<u16>()
        && (1..=24).contains(&n)
    {
        return Some(Vk(0x6F + n));
    }
    NAMED_KEYS
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, v)| Vk(*v))
}

fn key_name(vk: Vk) -> String {
    let v = vk.0;
    if (0x30..=0x39).contains(&v) || (0x41..=0x5A).contains(&v) {
        return (v as u8 as char).to_string();
    }
    if (0x70..=0x87).contains(&v) {
        return format!("F{}", v - 0x6F);
    }
    if let Some((n, _)) = NAMED_KEYS.iter().find(|(_, c)| *c == v) {
        return (*n).to_string();
    }
    format!("0x{v:02X}")
}

impl FromStr for KeyCombo {
    type Err = KeyParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        if s.is_empty() {
            return Err(KeyParseError::Empty);
        }
        let mut mods = Vec::new();
        let mut key: Option<(String, Vk)> = None;
        for raw in s.split('+') {
            let part = raw.trim();
            if part.is_empty() {
                return Err(KeyParseError::UnknownKey(String::new()));
            }
            let lower = part.to_ascii_lowercase();
            let canon = ALIASES
                .iter()
                .find(|(a, _)| *a == lower)
                .map_or(part, |(_, c)| *c);
            if let Some(m) = modifier_of(canon) {
                mods.push(m);
            } else if let Some(k) = key_of(canon) {
                if let Some((prev, _)) = &key {
                    return Err(KeyParseError::MultipleKeys(prev.clone(), part.to_string()));
                }
                key = Some((part.to_string(), k));
            } else {
                return Err(KeyParseError::UnknownKey(part.to_string()));
            }
        }
        let (_, key) = key.ok_or(KeyParseError::MissingKey)?;
        Ok(KeyCombo::new(mods, key))
    }
}

impl fmt::Display for KeyCombo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for m in &self.modifiers {
            let name = MODIFIER_ORDER
                .iter()
                .find(|(v, _)| v == m)
                .map_or_else(|| format!("0x{:02X}", m.0), |(_, n)| (*n).to_string());
            write!(f, "{name}+")?;
        }
        write!(f, "{}", key_name(self.key))
    }
}

/// Keys that need `KEYEVENTF_EXTENDEDKEY` to be interpreted correctly.
fn is_extended(vk: Vk) -> bool {
    matches!(
        vk.0,
        0x21..=0x28 // PageUp..Down, End, Home, arrows
        | 0x2C..=0x2E // PrintScreen, Insert, Delete
        | 0x5B | 0x5C | 0x5D // Win keys, Apps
        | 0x6F // numpad divide
        | 0x90 // NumLock
        | 0xA3 | 0xA5 // right Ctrl / right Alt
        | 0xA6..=0xB7 // browser / volume / media / launch keys
    )
}

fn key_input(vk: Vk, up: bool) -> INPUT {
    let mut flags = KEYBD_EVENT_FLAGS(0);
    if up {
        flags |= KEYEVENTF_KEYUP;
    }
    if is_extended(vk) {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    // SAFETY: MapVirtualKeyW has no preconditions.
    let scan = unsafe { MapVirtualKeyW(vk.0 as u32, MAPVK_VK_TO_VSC) } as u16;
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(vk.0),
                wScan: scan,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: YUBI_INJECT_MARK,
            },
        },
    }
}

/// Inputs for pressing (`down == true`: modifiers then key) or releasing
/// (`down == false`: key, then modifiers in reverse order) the combo.
/// Every event carries `dwExtraInfo = YUBI_INJECT_MARK`.
pub fn to_inputs(combo: &KeyCombo, down: bool) -> Vec<INPUT> {
    if down {
        combo
            .modifiers
            .iter()
            .map(|m| key_input(*m, false))
            .chain(std::iter::once(key_input(combo.key, false)))
            .collect()
    } else {
        std::iter::once(key_input(combo.key, true))
            .chain(combo.modifiers.iter().rev().map(|m| key_input(*m, true)))
            .collect()
    }
}

/// Inject events with `SendInput`. Returns false if not all were accepted
/// (e.g. blocked by UIPI when an elevated window is in the foreground).
pub fn send(inputs: &[INPUT]) -> bool {
    if inputs.is_empty() {
        return true;
    }
    // SAFETY: `inputs` is a valid slice of INPUT; size matches the struct.
    let n = unsafe { SendInput(inputs, std::mem::size_of::<INPUT>() as i32) };
    n as usize == inputs.len()
}

/// Tap the whole combo (press then release) in one `SendInput` call.
pub fn send_combo(combo: &KeyCombo) -> bool {
    let mut v = to_inputs(combo, true);
    v.extend(to_inputs(combo, false));
    send(&v)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> KeyCombo {
        s.parse().unwrap()
    }

    #[test]
    fn parses_basic() {
        assert_eq!(p("Alt+Tab"), KeyCombo::new([VK_MENU], Vk(0x09)));
        assert_eq!(
            p("Ctrl+Shift+T"),
            KeyCombo::new([VK_CONTROL, VK_SHIFT], Vk(b'T' as u16))
        );
        assert_eq!(p("Win+Left"), KeyCombo::new([VK_LWIN], Vk(0x25)));
        assert_eq!(p("F5").key, Vk(0x74));
        assert_eq!(p("f12").key, Vk(0x7B));
        assert_eq!(p("F24").key, Vk(0x87));
        assert_eq!(p("VolumeUp").key, Vk(0xAF));
        assert_eq!(p("MediaPlayPause").key, Vk(0xB3));
        assert_eq!(p("ctrl+f").key, Vk(b'F' as u16));
        assert_eq!(p("Ctrl+5").key, Vk(b'5' as u16));
    }

    #[test]
    fn aliases_and_whitespace() {
        assert_eq!(p(" control + shift + esc "), p("Ctrl+Shift+Esc"));
        assert_eq!(p("Super+Return"), p("Win+Enter"));
        assert_eq!(p("Ctrl+PgDn"), p("Ctrl+PageDown"));
    }

    #[test]
    fn modifier_order_is_normalised() {
        assert_eq!(p("Shift+Ctrl+A"), p("Ctrl+Shift+A"));
        assert_eq!(
            p("Win+Alt+Ctrl+Shift+A").to_string(),
            "Ctrl+Alt+Shift+Win+A"
        );
        assert_eq!(p("Ctrl+Ctrl+A"), p("Ctrl+A"));
    }

    #[test]
    fn errors_are_japanese() {
        assert_eq!("".parse::<KeyCombo>(), Err(KeyParseError::Empty));
        assert_eq!(
            "Ctrl+Shift".parse::<KeyCombo>(),
            Err(KeyParseError::MissingKey)
        );
        assert_eq!(
            "Ctrl+Foo".parse::<KeyCombo>(),
            Err(KeyParseError::UnknownKey("Foo".into()))
        );
        assert!(matches!(
            "A+B".parse::<KeyCombo>(),
            Err(KeyParseError::MultipleKeys(..))
        ));
        assert!("Ctrl++".parse::<KeyCombo>().is_err());
        let msg = "Ctrl+Foo".parse::<KeyCombo>().unwrap_err().to_string();
        assert!(msg.contains("認識できない"));
    }

    #[test]
    fn display_round_trips() {
        for s in [
            "Alt+Tab",
            "Ctrl+Shift+T",
            "Win+Left",
            "F5",
            "Ctrl+F",
            "VolumeUp",
            "MediaPlayPause",
            "Ctrl+Alt+Delete",
            "Ctrl+Plus",
            "Ctrl+Minus",
            "Shift+Win+S",
            "F13",
            "Ctrl+0",
        ] {
            let c = p(s);
            assert_eq!(c.to_string(), s);
            assert_eq!(p(&c.to_string()), c);
        }
    }

    #[test]
    fn every_named_key_round_trips() {
        for (name, vk) in NAMED_KEYS {
            let c = p(name);
            assert_eq!(c.key, Vk(*vk));
            assert_eq!(c.to_string(), *name);
        }
    }

    #[test]
    fn inputs_order_flags_and_mark() {
        let c = p("Ctrl+Shift+Left");
        let down = to_inputs(&c, true);
        let up = to_inputs(&c, false);
        assert_eq!(down.len(), 3);
        assert_eq!(up.len(), 3);
        // SAFETY: every INPUT here was built as a keyboard input.
        let ki = |i: &INPUT| unsafe { i.Anonymous.ki };
        assert_eq!(ki(&down[0]).wVk.0, 0x11);
        assert_eq!(ki(&down[1]).wVk.0, 0x10);
        assert_eq!(ki(&down[2]).wVk.0, 0x25);
        assert_eq!(ki(&up[0]).wVk.0, 0x25);
        assert_eq!(ki(&up[2]).wVk.0, 0x11);
        for i in down.iter().chain(up.iter()) {
            assert_eq!(i.r#type, INPUT_KEYBOARD);
            assert_eq!(ki(i).dwExtraInfo, YUBI_INJECT_MARK);
        }
        assert!(ki(&down[2]).dwFlags.contains(KEYEVENTF_EXTENDEDKEY));
        assert!(!ki(&down[0]).dwFlags.contains(KEYEVENTF_EXTENDEDKEY));
        assert!(!ki(&down[2]).dwFlags.contains(KEYEVENTF_KEYUP));
        assert!(ki(&up[0]).dwFlags.contains(KEYEVENTF_KEYUP));
    }
}
