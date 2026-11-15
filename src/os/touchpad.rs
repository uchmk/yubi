//! Windows' own touchpad settings (`SPI_GETTOUCHPADPARAMETERS` / `SPI_SETTOUCHPADPARAMETERS`).
//!
//! Windows keeps a cursor speed for the touchpad that is separate from the mouse pointer speed
//! (Settings > Bluetooth とデバイス > タッチパッド > カーソルの速度). yubi reads and switches it; the
//! pointer motion itself stays Windows'. The registry is never touched: where the call is
//! refused the setting is simply unavailable.
//!
//! The `windows` crate has no binding for these, so the V1 struct is declared here from
//! `WinUser.h` (Windows SDK 10.0.26100, `TOUCHPAD_PARAMETERS_V1`, 44 bytes). The header does not
//! document the speed range; [`SPEED_MIN`]..=[`SPEED_MAX`] is what Windows accepts (verified on
//! hardware: 0 reads back as the default 10, values above 20 read back as 20).

use std::sync::OnceLock;

/// `TOUCHPAD_PARAMETERS_VERSION_1`.
const VERSION_1: u32 = 1;

/// Lowest cursor speed Windows accepts.
pub const SPEED_MIN: u8 = 1;
/// Highest cursor speed Windows accepts.
pub const SPEED_MAX: u8 = 20;
/// What Windows ships as the default (the middle of the Settings slider).
pub const SPEED_DEFAULT: u8 = 10;

/// Bit positions in the user-settings bitfield of `TOUCHPAD_PARAMETERS_V1` (declaration order,
/// from bit 0): `allowActiveWhenMousePresent`, `feedbackEnabled`, `tapEnabled`,
/// `tapAndDragEnabled`, `twoFingerTapEnabled`, `rightClickZoneEnabled`, `mouseAccelSettingHonored`,
/// `panEnabled`, ...
#[allow(dead_code)]
const PAN_ENABLED_BIT: u32 = 1 << 7;
/// First bit of the status bitfield: `touchpadPresent`.
const TOUCHPAD_PRESENT_BIT: u32 = 1 << 0;

/// `TOUCHPAD_PARAMETERS_V1` (`BOOL x : 1` bitfields packed into two `u32`s).
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Params {
    version: u32,
    max_supported_contacts: u32,
    legacy_features: u32,
    status_bits: u32,
    user_bits: u32,
    sensitivity_level: u32,
    cursor_speed: u32,
    feedback_intensity: u32,
    click_force_sensitivity: u32,
    right_click_zone_width: u32,
    right_click_zone_height: u32,
}

/// Why the touchpad settings cannot be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unsupported {
    /// The call or the struct version was refused (Windows older than 11 24H2, or not Windows).
    Refused,
    /// No precision touchpad is present.
    NoTouchpad,
}

impl Unsupported {
    /// Short Japanese reason for the UI.
    pub fn reason(&self) -> &'static str {
        match self {
            Self::Refused => {
                "この Windows ではタッチパッドの設定を読み書きできません (Windows 11 24H2 以降が必要です)。"
            }
            Self::NoTouchpad => "Windows がタッチパッドを認識していません。",
        }
    }
}

#[cfg(windows)]
fn call(set: bool, p: &mut Params, persist: bool) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{
        SPIF_SENDCHANGE, SPIF_UPDATEINIFILE, SYSTEM_PARAMETERS_INFO_ACTION,
        SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW,
    };
    const SPI_GETTOUCHPADPARAMETERS: SYSTEM_PARAMETERS_INFO_ACTION =
        SYSTEM_PARAMETERS_INFO_ACTION(0x00AE);
    const SPI_SETTOUCHPADPARAMETERS: SYSTEM_PARAMETERS_INFO_ACTION =
        SYSTEM_PARAMETERS_INFO_ACTION(0x00AF);
    let (action, flags) = if set {
        let f = if persist {
            SPIF_UPDATEINIFILE | SPIF_SENDCHANGE
        } else {
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0)
        };
        (SPI_SETTOUCHPADPARAMETERS, f)
    } else {
        (
            SPI_GETTOUCHPADPARAMETERS,
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    p.version = VERSION_1;
    // SAFETY: `p` is a live, correctly sized `TOUCHPAD_PARAMETERS_V1`; `uiParam` is its size.
    unsafe {
        SystemParametersInfoW(
            action,
            size_of::<Params>() as u32,
            Some((p as *mut Params).cast()),
            flags,
        )
    }
    .is_ok()
}

#[cfg(not(windows))]
fn call(_: bool, _: &mut Params, _: bool) -> bool {
    false
}

fn get() -> Option<Params> {
    let mut p = Params::default();
    call(false, &mut p, false).then_some(p)
}

/// Whether the cursor speed can be used on this machine (asked once, then cached). An env var
/// forces the unsupported state for screenshots.
pub fn support() -> Result<(), Unsupported> {
    static CACHE: OnceLock<Result<(), Unsupported>> = OnceLock::new();
    CACHE
        .get_or_init(|| {
            if std::env::var_os("YUBI_CURSOR_UNSUPPORTED").is_some() {
                return Err(Unsupported::Refused);
            }
            let p = get().ok_or(Unsupported::Refused)?;
            if p.status_bits & TOUCHPAD_PRESENT_BIT == 0 {
                return Err(Unsupported::NoTouchpad);
            }
            Ok(())
        })
        .clone()
}

/// The touchpad cursor speed Windows uses right now (`None` when unsupported).
pub fn cursor_speed() -> Option<u8> {
    support().ok()?;
    get().map(|p| p.cursor_speed.clamp(0, u32::from(u8::MAX)) as u8)
}

/// Set the touchpad cursor speed. `persist` writes it to the user's settings (what the user
/// chose); without it the value only lasts until the next change or sign-out. Returns whether
/// Windows accepted it.
pub fn set_cursor_speed(speed: u8, persist: bool) -> bool {
    if support().is_err() {
        return false;
    }
    let Some(mut p) = get() else {
        return false;
    };
    p.cursor_speed = u32::from(speed.clamp(SPEED_MIN, SPEED_MAX));
    call(true, &mut p, persist)
}

/// Whether Windows' two-finger scrolling (panning) is on. `None` when unsupported.
#[allow(dead_code)] // for the two-finger scroll feature
pub fn two_finger_scroll_enabled() -> Option<bool> {
    support().ok()?;
    get().map(|p| p.user_bits & PAN_ENABLED_BIT != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn struct_matches_the_sdk_layout() {
        // TOUCHPAD_PARAMETERS_V1: 3 UINT + 2 bitfield words + 6 UINT.
        assert_eq!(size_of::<Params>(), 44);
    }

    /// Hardware check: `cargo test touchpad_hw -- --ignored --nocapture`. Restores the value.
    #[test]
    #[ignore]
    fn touchpad_hw() {
        let Some(orig) = cursor_speed() else {
            eprintln!("unsupported: {:?}", support());
            return;
        };
        eprintln!(
            "original speed {orig}, pan {:?}",
            two_finger_scroll_enabled()
        );
        for v in [0u8, 1, 2, 10, 19, 20, 21, 255] {
            let mut p = get().unwrap();
            p.cursor_speed = u32::from(v);
            let ok = call(true, &mut p, false);
            eprintln!("set {v}: ok={ok}, read back {:?}", cursor_speed());
        }
        let mut p = get().unwrap();
        p.cursor_speed = u32::from(orig);
        assert!(call(true, &mut p, false));
        assert_eq!(cursor_speed(), Some(orig));
    }
}
