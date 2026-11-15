//! Pure decision core of the mouse hook. No Win32 calls: everything is driven by [`LlEvent`]s
//! with caller-supplied timestamps so the logic is unit-testable.

use crate::os::YUBI_INJECT_MARK;

/// Default for how long `FreezeCursor` may stay set before the hook drops back to `Off`.
pub const DEFAULT_MAX_FREEZE_MS: u64 = 10_000;

/// Default lease: a suppression mode expires this long after the last `Hook::renew`.
pub const DEFAULT_LEASE_MS: u64 = 250;

/// Mouse buttons the hook distinguishes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Button {
    Left,
    Right,
    Middle,
    X1,
    X2,
}

impl Button {
    /// Bit of this button in a hold mask.
    pub const fn bit(self) -> u8 {
        match self {
            Button::Left => 1,
            Button::Right => 2,
            Button::Middle => 4,
            Button::X1 => 8,
            Button::X2 => 16,
        }
    }

    /// All buttons (mask `0x1F`).
    pub const ALL: u8 = 0x1F;

    /// All buttons in the order of their bits.
    pub const EACH: [Button; 5] = [
        Button::Left,
        Button::Right,
        Button::Middle,
        Button::X1,
        Button::X2,
    ];
}

/// What the hook should suppress right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Mode {
    /// Pass everything.
    #[default]
    Off,
    /// Swallow non-injected moves.
    FreezeCursor,
    /// Swallow (and report) non-injected button downs of the hold set.
    HoldClicks,
    /// Both of the above.
    FreezeAndHold,
}

impl Mode {
    pub const fn freezes(self) -> bool {
        matches!(self, Mode::FreezeCursor | Mode::FreezeAndHold)
    }

    pub const fn holds(self) -> bool {
        matches!(self, Mode::HoldClicks | Mode::FreezeAndHold)
    }

    /// The same mode with the cursor freeze removed.
    pub const fn without_freeze(self) -> Mode {
        if self.holds() {
            Mode::HoldClicks
        } else {
            Mode::Off
        }
    }

    pub(super) const fn to_u8(self) -> u8 {
        match self {
            Mode::Off => 0,
            Mode::FreezeCursor => 1,
            Mode::HoldClicks => 2,
            Mode::FreezeAndHold => 3,
        }
    }

    pub(super) const fn from_u8(v: u8) -> Mode {
        match v {
            1 => Mode::FreezeCursor,
            2 => Mode::HoldClicks,
            3 => Mode::FreezeAndHold,
            _ => Mode::Off,
        }
    }
}

/// Kind of low-level mouse event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LlKind {
    Move,
    Down(Button),
    Up(Button),
    /// Wheel and anything else: always passes.
    Other,
}

/// One `WH_MOUSE_LL` event, reduced to what the decision needs.
#[derive(Clone, Copy, Debug)]
pub struct LlEvent {
    pub kind: LlKind,
    /// `LLMHF_INJECTED` / `LLMHF_LOWER_IL_INJECTED` set.
    pub injected: bool,
    pub extra: usize,
    /// Monotonic microseconds.
    pub t_us: i64,
}

/// Longest continuous freeze even with live touches (a slow volume slide stays far below it).
pub const ABS_MAX_FREEZE_US: i64 = 120_000_000;

/// Button ledger (who decides what, for each button event):
///
/// * **Forwarded down**: a non-yubi down that arrives while the mode does not hold it (Off,
///   `FreezeCursor`, lease expired, watchdog tripped, button outside the hold set). It reaches
///   the OS, so its up must reach the OS too: the up passes whatever the mode is by then,
///   because `held` has no bit for it.
/// * **Swallowed down**: a non-yubi down eaten by `HoldClicks`. Its bit is set in `held`, and
///   its up is swallowed whatever the mode is by then (Off, lease expiry, watchdog, emergency).
/// * **Injected by yubi / other software**: `injected` or `extra == YUBI_INJECT_MARK`. Always
///   passes and never touches `held`.
///
/// What the callback does with the event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Pass,
    Swallow,
    /// Swallow and emit `ButtonHeld` (down) / `ButtonReleased` (up).
    SwallowAndReport,
}

/// Mutable state carried between events (owned by the hook thread).
#[derive(Clone, Debug)]
pub struct HookState {
    /// Buttons whose down was swallowed and whose up is still owed a swallow.
    pub held: u8,
    /// Buttons `HoldClicks` applies to.
    pub hold_mask: u8,
    /// First event time while a freeze mode was active.
    freeze_since: Option<i64>,
    /// Same start, but never moved by a live-touch ping: the absolute cap counts from here.
    freeze_abs_since: Option<i64>,
    pub max_freeze_us: i64,
    /// Longest a freeze may last even while the engine keeps proving a live touch.
    pub abs_max_freeze_us: i64,
    /// Time of the engine's last "a finger is on the pad" ping ([`i64::MIN`] = never). The idle
    /// window of the watchdog counts from the later of the freeze start and this.
    pub last_alive_us: i64,
    /// Set by `decide` when the freeze exceeded the limit; the caller must switch to `Off`,
    /// report `Watchdog` and call [`HookState::ack_watchdog`].
    pub watchdog_tripped: bool,
    /// Set by `decide` when the lease ran out while a mode was active; the caller must switch
    /// to `Off`, report `LeaseExpired` and call [`HookState::ack_lease`].
    pub lease_expired: bool,
}

impl HookState {
    pub fn new(hold_mask: u8, max_freeze_ms: u64) -> Self {
        Self {
            held: 0,
            hold_mask,
            freeze_since: None,
            freeze_abs_since: None,
            max_freeze_us: (max_freeze_ms.min(i64::MAX as u64 / 1000) * 1000) as i64,
            abs_max_freeze_us: ABS_MAX_FREEZE_US,
            last_alive_us: i64::MIN,
            watchdog_tripped: false,
            lease_expired: false,
        }
    }

    /// Forget the freeze start (call when the mode was changed from outside).
    pub fn reset_freeze(&mut self) {
        self.freeze_since = None;
        self.freeze_abs_since = None;
    }

    pub fn ack_lease(&mut self) {
        self.lease_expired = false;
        self.freeze_since = None;
        self.freeze_abs_since = None;
    }

    pub fn ack_watchdog(&mut self) {
        self.watchdog_tripped = false;
        self.freeze_since = None;
        self.freeze_abs_since = None;
    }
}

/// Decide what to do with one event. Allocation-free and lock-free.
///
/// `lease_until_us`: a non-`Off` mode is only honoured while `ev.t_us <= lease_until_us`;
/// afterwards it is treated as `Off` (held ups are still swallowed) and `lease_expired` is set.
pub fn decide(state: &mut HookState, mode: Mode, lease_until_us: i64, ev: LlEvent) -> Verdict {
    let mut mode = mode;
    if mode != Mode::Off && ev.t_us > lease_until_us {
        state.lease_expired = true;
        mode = Mode::Off;
    }
    // Safety valve: a freeze mode that outlives `max_freeze_us` is cancelled.
    if mode.freezes() {
        let since = *state.freeze_since.get_or_insert(ev.t_us);
        let abs_since = *state.freeze_abs_since.get_or_insert(ev.t_us);
        // A live touch proves the freeze is still wanted: it restarts the idle window. The
        // absolute cap is the last net for a stuck engine that fakes live touches.
        let idle_since = since.max(state.last_alive_us);
        if ev.t_us - idle_since > state.max_freeze_us
            || ev.t_us - abs_since > state.abs_max_freeze_us.max(state.max_freeze_us)
        {
            state.watchdog_tripped = true;
        }
    } else {
        state.freeze_since = None;
        state.freeze_abs_since = None;
    }
    if state.watchdog_tripped {
        mode = Mode::Off;
    }

    let ours = ev.injected || ev.extra == YUBI_INJECT_MARK;
    match ev.kind {
        LlKind::Move => {
            if mode.freezes() && !ours {
                Verdict::Swallow
            } else {
                Verdict::Pass
            }
        }
        LlKind::Down(b) => {
            let bit = b.bit();
            if mode.holds() && !ours && state.hold_mask & bit != 0 {
                state.held |= bit;
                Verdict::SwallowAndReport
            } else {
                // A fresh physical down that passes cancels any stale held bit. Our own and
                // other software's injected downs say nothing about the physical button.
                if !ours {
                    state.held &= !bit;
                }
                Verdict::Pass
            }
        }
        LlKind::Up(b) => {
            let bit = b.bit();
            // Never let an up through whose down we ate, whatever the mode is now.
            if state.held & bit != 0 && !ours {
                state.held &= !bit;
                Verdict::SwallowAndReport
            } else {
                Verdict::Pass
            }
        }
        LlKind::Other => Verdict::Pass,
    }
}

/// Modifier bits for [`EmergencyCombo::mods`].
pub const MOD_CTRL: u8 = 1;
pub const MOD_ALT: u8 = 2;
pub const MOD_SHIFT: u8 = 4;
pub const MOD_WIN: u8 = 8;

pub const VK_ESCAPE: u32 = 0x1B;

/// The emergency-release key combination (default Ctrl+Alt+Esc).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EmergencyCombo {
    pub vk: u32,
    pub mods: u8,
}

impl Default for EmergencyCombo {
    fn default() -> Self {
        Self {
            vk: VK_ESCAPE,
            mods: MOD_CTRL | MOD_ALT,
        }
    }
}

/// `(bit in KbState::down, MOD_* group)` of a modifier virtual key; left and right keys have
/// separate bits so releasing one while the other is held keeps the modifier down.
const fn modifier_slot(vk: u32) -> Option<(u8, u8)> {
    match vk {
        0x11 | 0xA2 => Some((1, MOD_CTRL)),
        0xA3 => Some((2, MOD_CTRL)),
        0x12 | 0xA4 => Some((4, MOD_ALT)),
        0xA5 => Some((8, MOD_ALT)),
        0x10 | 0xA0 => Some((16, MOD_SHIFT)),
        0xA1 => Some((32, MOD_SHIFT)),
        0x5B => Some((64, MOD_WIN)),
        0x5C => Some((128, MOD_WIN)),
        _ => None,
    }
}

/// Keyboard state for emergency detection (owned by the hook thread).
#[derive(Clone, Copy, Debug, Default)]
pub struct KbState {
    /// Pressed modifier keys, one bit per physical key (left/right separate).
    down: u8,
    /// The trigger key is down and already fired (so auto-repeat fires once per press).
    fired: bool,
}

impl KbState {
    fn mods(&self) -> u8 {
        let mut m = 0;
        for vk in [0xA2, 0xA3, 0xA4, 0xA5, 0xA0, 0xA1, 0x5B, 0x5C] {
            if let Some((bit, group)) = modifier_slot(vk)
                && self.down & bit != 0
            {
                m |= group;
            }
        }
        m
    }

    /// Feed one `WH_KEYBOARD_LL` event; true exactly when the combo is pressed (once per press
    /// of the trigger key, auto-repeat ignored, modifier order irrelevant). The caller never
    /// swallows the key.
    pub fn on_key(&mut self, combo: EmergencyCombo, vk: u32, down: bool) -> bool {
        if let Some((bit, _)) = modifier_slot(vk) {
            if down {
                self.down |= bit;
            } else {
                self.down &= !bit;
            }
            return false;
        }
        if vk != combo.vk {
            return false;
        }
        if !down {
            self.fired = false;
            return false;
        }
        if self.fired {
            return false;
        }
        if self.mods() & combo.mods == combo.mods {
            self.fired = true;
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(kind: LlKind, t_us: i64) -> LlEvent {
        LlEvent {
            kind,
            injected: false,
            extra: 0,
            t_us,
        }
    }

    const FAR: i64 = i64::MAX;

    fn st() -> HookState {
        HookState::new(Button::ALL, DEFAULT_MAX_FREEZE_MS)
    }

    #[test]
    fn moves_swallowed_only_in_freeze_modes() {
        let mut s = st();
        for (m, want) in [
            (Mode::Off, Verdict::Pass),
            (Mode::HoldClicks, Verdict::Pass),
            (Mode::FreezeCursor, Verdict::Swallow),
            (Mode::FreezeAndHold, Verdict::Swallow),
        ] {
            assert_eq!(decide(&mut s, m, FAR, ev(LlKind::Move, 0)), want, "{m:?}");
        }
    }

    #[test]
    fn injected_and_marked_always_pass() {
        let mut s = st();
        let mut e = ev(LlKind::Move, 0);
        e.injected = true;
        assert_eq!(decide(&mut s, Mode::FreezeAndHold, FAR, e), Verdict::Pass);
        let mut e = ev(LlKind::Down(Button::Left), 0);
        e.extra = YUBI_INJECT_MARK;
        assert_eq!(decide(&mut s, Mode::FreezeAndHold, FAR, e), Verdict::Pass);
        let mut e = ev(LlKind::Down(Button::Left), 0);
        e.injected = true;
        assert_eq!(decide(&mut s, Mode::HoldClicks, FAR, e), Verdict::Pass);
        assert_eq!(s.held, 0);
    }

    #[test]
    fn held_down_swallows_its_up_even_after_off() {
        let mut s = st();
        let l = Button::Left;
        assert_eq!(
            decide(&mut s, Mode::HoldClicks, FAR, ev(LlKind::Down(l), 0)),
            Verdict::SwallowAndReport
        );
        assert_eq!(
            decide(&mut s, Mode::Off, FAR, ev(LlKind::Up(l), 1)),
            Verdict::SwallowAndReport
        );
        // The up is consumed once; a second one passes.
        assert_eq!(
            decide(&mut s, Mode::Off, FAR, ev(LlKind::Up(l), 2)),
            Verdict::Pass
        );
    }

    #[test]
    fn injected_down_does_not_clear_held() {
        let mut s = st();
        let l = Button::Left;
        let d = ev(LlKind::Down(l), 0);
        assert_eq!(
            decide(&mut s, Mode::HoldClicks, FAR, d),
            Verdict::SwallowAndReport
        );
        let mut m = ev(LlKind::Down(l), 1);
        m.extra = YUBI_INJECT_MARK;
        assert_eq!(decide(&mut s, Mode::HoldClicks, FAR, m), Verdict::Pass);
        let u = ev(LlKind::Up(l), 2);
        assert_eq!(decide(&mut s, Mode::Off, FAR, u), Verdict::SwallowAndReport);
    }

    #[test]
    fn up_without_swallowed_down_passes() {
        let mut s = st();
        assert_eq!(
            decide(
                &mut s,
                Mode::FreezeAndHold,
                FAR,
                ev(LlKind::Up(Button::Right), 0)
            ),
            Verdict::Pass
        );
    }

    #[test]
    fn buttons_outside_hold_set_pass() {
        let mut s = HookState::new(Button::Left.bit(), DEFAULT_MAX_FREEZE_MS);
        assert_eq!(
            decide(
                &mut s,
                Mode::HoldClicks,
                FAR,
                ev(LlKind::Down(Button::Right), 0)
            ),
            Verdict::Pass
        );
        assert_eq!(
            decide(
                &mut s,
                Mode::HoldClicks,
                FAR,
                ev(LlKind::Up(Button::Right), 1)
            ),
            Verdict::Pass
        );
        assert_eq!(
            decide(
                &mut s,
                Mode::HoldClicks,
                FAR,
                ev(LlKind::Down(Button::Left), 2)
            ),
            Verdict::SwallowAndReport
        );
    }

    #[test]
    fn freeze_only_mode_does_not_hold_clicks() {
        let mut s = st();
        assert_eq!(
            decide(
                &mut s,
                Mode::FreezeCursor,
                FAR,
                ev(LlKind::Down(Button::Left), 0)
            ),
            Verdict::Pass
        );
    }

    #[test]
    fn wheel_and_other_pass() {
        let mut s = st();
        assert_eq!(
            decide(&mut s, Mode::FreezeAndHold, FAR, ev(LlKind::Other, 0)),
            Verdict::Pass
        );
    }

    #[test]
    fn watchdog_trips_after_max_freeze() {
        let mut s = HookState::new(Button::ALL, 100);
        assert_eq!(
            decide(&mut s, Mode::FreezeCursor, FAR, ev(LlKind::Move, 0)),
            Verdict::Swallow
        );
        assert_eq!(
            decide(&mut s, Mode::FreezeCursor, FAR, ev(LlKind::Move, 100_000)),
            Verdict::Swallow
        );
        assert!(!s.watchdog_tripped);
        assert_eq!(
            decide(&mut s, Mode::FreezeCursor, FAR, ev(LlKind::Move, 100_001)),
            Verdict::Pass
        );
        assert!(s.watchdog_tripped);
        s.ack_watchdog();
        // Caller switched to Off; a new freeze starts a new window.
        assert_eq!(
            decide(&mut s, Mode::Off, FAR, ev(LlKind::Move, 200_000)),
            Verdict::Pass
        );
        assert_eq!(
            decide(&mut s, Mode::FreezeCursor, FAR, ev(LlKind::Move, 300_000)),
            Verdict::Swallow
        );
    }

    fn run_pings(s: &mut HookState, ping: bool, from_us: i64, until_us: i64) -> Option<i64> {
        // A move every 100 ms; a ping (at the same time) when `ping`.
        let mut t = from_us;
        while t <= until_us {
            if ping {
                s.last_alive_us = t;
            }
            decide(s, Mode::FreezeCursor, i64::MAX, ev(LlKind::Move, t));
            if s.watchdog_tripped {
                return Some(t);
            }
            t += 100_000;
        }
        None
    }

    #[test]
    fn alive_pings_keep_a_long_freeze() {
        let mut s = HookState::new(Button::ALL, 10_000);
        assert_eq!(run_pings(&mut s, true, 0, 30_000_000), None);
    }

    #[test]
    fn freeze_without_pings_trips_at_ten_seconds() {
        let mut s = HookState::new(Button::ALL, 10_000);
        let t = run_pings(&mut s, false, 0, 30_000_000).unwrap();
        assert!((10_000_000..10_200_000).contains(&t), "{t}");
    }

    #[test]
    fn pings_stopping_trip_ten_seconds_after_the_last() {
        let mut s = HookState::new(Button::ALL, 10_000);
        assert_eq!(run_pings(&mut s, true, 0, 20_000_000), None);
        let t = run_pings(&mut s, false, 20_100_000, 40_000_000).unwrap();
        assert!((30_000_000..30_300_000).contains(&t), "{t}");
    }

    #[test]
    fn alive_pings_do_not_beat_the_absolute_cap() {
        let mut s = HookState::new(Button::ALL, 10_000);
        let t = run_pings(&mut s, true, 0, 200_000_000).unwrap();
        assert!(
            (ABS_MAX_FREEZE_US..ABS_MAX_FREEZE_US + 200_000).contains(&t),
            "{t}"
        );
    }

    #[test]
    fn leaving_freeze_resets_the_window() {
        let mut s = HookState::new(Button::ALL, 100);
        decide(&mut s, Mode::FreezeCursor, FAR, ev(LlKind::Move, 0));
        decide(&mut s, Mode::Off, FAR, ev(LlKind::Move, 50_000));
        assert_eq!(
            decide(&mut s, Mode::FreezeCursor, FAR, ev(LlKind::Move, 500_000)),
            Verdict::Swallow
        );
        assert!(!s.watchdog_tripped);
    }

    #[test]
    fn mode_u8_roundtrip() {
        for m in [
            Mode::Off,
            Mode::FreezeCursor,
            Mode::HoldClicks,
            Mode::FreezeAndHold,
        ] {
            assert_eq!(Mode::from_u8(m.to_u8()), m);
        }
    }

    #[test]
    fn x1_and_x2_are_tracked_separately() {
        let mut s = st();
        let d1 = ev(LlKind::Down(Button::X1), 0);
        assert_eq!(
            decide(&mut s, Mode::HoldClicks, FAR, d1),
            Verdict::SwallowAndReport
        );
        // X2 down passes in Off and must not clear X1's held bit.
        let d2 = ev(LlKind::Down(Button::X2), 1);
        assert_eq!(decide(&mut s, Mode::Off, FAR, d2), Verdict::Pass);
        let u1 = ev(LlKind::Up(Button::X1), 2);
        assert_eq!(
            decide(&mut s, Mode::Off, FAR, u1),
            Verdict::SwallowAndReport
        );
        assert_eq!(s.held, 0);
    }

    #[test]
    fn up_still_swallowed_after_watchdog_trip() {
        let mut s = HookState::new(Button::ALL, 100);
        let d = ev(LlKind::Down(Button::Left), 0);
        assert_eq!(
            decide(&mut s, Mode::FreezeAndHold, FAR, d),
            Verdict::SwallowAndReport
        );
        let m = ev(LlKind::Move, 200_000);
        assert_eq!(decide(&mut s, Mode::FreezeAndHold, FAR, m), Verdict::Pass);
        assert!(s.watchdog_tripped);
        s.ack_watchdog();
        let u = ev(LlKind::Up(Button::Left), 200_001);
        assert_eq!(decide(&mut s, Mode::Off, FAR, u), Verdict::SwallowAndReport);
    }

    #[test]
    fn lease_expires_modes_but_not_held_ups() {
        let mut s = st();
        let lease = 250_000;
        let m = ev(LlKind::Move, 250_000);
        assert_eq!(
            decide(&mut s, Mode::FreezeAndHold, lease, m),
            Verdict::Swallow
        );
        assert!(!s.lease_expired);
        let d = ev(LlKind::Down(Button::Left), 100);
        assert_eq!(
            decide(&mut s, Mode::FreezeAndHold, lease, d),
            Verdict::SwallowAndReport
        );
        // Past the lease: move and new down pass, flag set.
        let m = ev(LlKind::Move, 250_001);
        assert_eq!(decide(&mut s, Mode::FreezeAndHold, lease, m), Verdict::Pass);
        assert!(s.lease_expired);
        s.ack_lease();
        let d2 = ev(LlKind::Down(Button::Right), 250_002);
        assert_eq!(decide(&mut s, Mode::HoldClicks, lease, d2), Verdict::Pass);
        // The held up is still swallowed.
        let u = ev(LlKind::Up(Button::Left), 250_003);
        assert_eq!(
            decide(&mut s, Mode::Off, lease, u),
            Verdict::SwallowAndReport
        );
    }

    #[test]
    fn renewed_lease_keeps_mode_and_off_never_expires() {
        let mut s = st();
        let m = ev(LlKind::Move, 900_000);
        assert_eq!(
            decide(&mut s, Mode::FreezeCursor, 1_000_000, m),
            Verdict::Swallow
        );
        assert!(!s.lease_expired);
        // Off past any lease: nothing to report.
        let m = ev(LlKind::Move, 5_000_000);
        assert_eq!(decide(&mut s, Mode::Off, 0, m), Verdict::Pass);
        assert!(!s.lease_expired);
    }

    #[test]
    fn forwarded_down_up_passes_after_mode_change() {
        let mut s = st();
        let l = Button::Left;
        assert_eq!(
            decide(&mut s, Mode::Off, FAR, ev(LlKind::Down(l), 0)),
            Verdict::Pass
        );
        assert_eq!(
            decide(&mut s, Mode::FreezeAndHold, FAR, ev(LlKind::Up(l), 1)),
            Verdict::Pass
        );
        assert_eq!(s.held, 0);
        // Same for a down forwarded because the lease had expired.
        let d = ev(LlKind::Down(l), 500);
        assert_eq!(decide(&mut s, Mode::HoldClicks, 100, d), Verdict::Pass);
        s.ack_lease();
        assert_eq!(
            decide(&mut s, Mode::HoldClicks, FAR, ev(LlKind::Up(l), 501)),
            Verdict::Pass
        );
    }

    fn feed(k: &mut KbState, keys: &[(u32, bool)]) -> Vec<bool> {
        let c = EmergencyCombo::default();
        keys.iter().map(|&(vk, d)| k.on_key(c, vk, d)).collect()
    }

    #[test]
    fn emergency_fires_in_any_modifier_order_once_per_press() {
        let mut k = KbState::default();
        let r = feed(
            &mut k,
            &[
                (0xA2, true),
                (0xA4, true),
                (VK_ESCAPE, true),
                (VK_ESCAPE, true),
                (VK_ESCAPE, true),
                (VK_ESCAPE, false),
                (VK_ESCAPE, true),
            ],
        );
        assert_eq!(r, [false, false, true, false, false, false, true]);
        // Alt first, right Ctrl.
        let mut k = KbState::default();
        let r = feed(&mut k, &[(0xA5, true), (0xA3, true), (VK_ESCAPE, true)]);
        assert_eq!(r, [false, false, true]);
        // Generic VK_MENU / VK_CONTROL.
        let mut k = KbState::default();
        let r = feed(&mut k, &[(0x12, true), (0x11, true), (VK_ESCAPE, true)]);
        assert_eq!(r, [false, false, true]);
    }

    #[test]
    fn emergency_needs_all_modifiers() {
        let mut k = KbState::default();
        assert_eq!(feed(&mut k, &[(VK_ESCAPE, true)]), [false]);
        let mut k = KbState::default();
        assert_eq!(
            feed(&mut k, &[(0xA2, true), (VK_ESCAPE, true)]),
            [false, false]
        );
        // A released Alt no longer counts; other keys never fire.
        let mut k = KbState::default();
        let r = feed(
            &mut k,
            &[
                (0xA2, true),
                (0xA4, true),
                (0xA4, false),
                (VK_ESCAPE, true),
                (0x41, true),
            ],
        );
        assert_eq!(r, [false, false, false, false, false]);
    }

    #[test]
    fn without_freeze_keeps_holding() {
        assert_eq!(Mode::FreezeAndHold.without_freeze(), Mode::HoldClicks);
        assert_eq!(Mode::FreezeCursor.without_freeze(), Mode::Off);
        assert_eq!(Mode::HoldClicks.without_freeze(), Mode::HoldClicks);
    }
}
