//! Executor: the only owner of keys and mouse buttons that yubi holds down.
//!
//! Gestures call `press` / `release`; the executor remembers what is down so that
//! `release_all` can always let go (device gone, engine disabled, config changed, lock, exit).
//! Everything goes through an `InputSink`, so the bookkeeping is testable without Win32.

/// Marks input injected by yubi (`dwExtraInfo`) so later hooks can let it through.
pub const YUBI_INJECT_MARK: usize = 0x5955_4249;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

/// A key or button that can be held down.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    /// Windows virtual-key code.
    Vk(u16),
    Mouse(MouseButton),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SynthEvent {
    Down(Key),
    Up(Key),
    /// Relative pointer movement in pixels.
    Move(i32, i32),
    /// Wheel movement in `WHEEL_DELTA` units (120 = one notch): horizontal, then vertical.
    /// Positive scrolls right / up.
    Wheel(i32, i32),
}

pub trait InputSink {
    /// Deliver one event. Returns false when the system refused it.
    fn send(&mut self, ev: SynthEvent) -> bool;
}

pub struct Executor<S: InputSink> {
    sink: S,
    /// In press order; released in reverse.
    held: Vec<Key>,
}

impl<S: InputSink> Executor<S> {
    pub fn new(sink: S) -> Self {
        Self {
            sink,
            held: Vec::new(),
        }
    }

    pub fn held(&self) -> &[Key] {
        &self.held
    }

    /// Press `key` unless it is already down. Returns false if the system refused.
    pub fn press(&mut self, key: Key) -> bool {
        if self.held.contains(&key) {
            return true;
        }
        let ok = self.sink.send(SynthEvent::Down(key));
        if ok {
            self.held.push(key);
        }
        ok
    }

    /// Release `key` if yubi holds it. A refused release stays owned so `release_all` retries it.
    pub fn release(&mut self, key: Key) -> bool {
        let Some(i) = self.held.iter().position(|k| *k == key) else {
            return true;
        };
        let ok = self.sink.send(SynthEvent::Up(key));
        if ok {
            self.held.remove(i);
        }
        ok
    }

    /// Move the pointer by (dx, dy) pixels. Returns false if the system refused.
    pub fn move_mouse(&mut self, dx: i32, dy: i32) -> bool {
        dx == 0 && dy == 0 || self.sink.send(SynthEvent::Move(dx, dy))
    }

    /// Turn the wheel by (dx, dy) `WHEEL_DELTA` units in one go. Returns false if refused.
    pub fn wheel(&mut self, dx: i32, dy: i32) -> bool {
        dx == 0 && dy == 0 || self.sink.send(SynthEvent::Wheel(dx, dy))
    }

    /// Turn the mouse wheel by `notches` (positive = up or right) on one axis. Returns false if
    /// refused.
    pub fn wheel_notches(&mut self, notches: i32, horizontal: bool) -> bool {
        let units = notches.saturating_mul(120);
        if horizontal {
            self.wheel(units, 0)
        } else {
            self.wheel(0, units)
        }
    }

    /// Press all `keys` in order, then release them in reverse (a shortcut or a click).
    /// Everything pressed here is owned by the executor, so a failure part-way lets go again.
    /// Keys that were already held stay held. Returns false if any press was refused.
    pub fn tap(&mut self, keys: &[Key]) -> bool {
        let mut mine = Vec::new();
        let mut ok = true;
        for &k in keys {
            let was_held = self.held.contains(&k);
            if !self.press(k) {
                ok = false;
                break;
            }
            if !was_held {
                mine.push(k);
            }
        }
        while let Some(k) = mine.pop() {
            ok &= self.release(k);
        }
        ok
    }

    /// Let go of everything yubi holds. Returns how many keys could not be released.
    pub fn release_all(&mut self) -> usize {
        let mut stuck = Vec::new();
        while let Some(key) = self.held.pop() {
            if !self.sink.send(SynthEvent::Up(key)) {
                stuck.push(key);
            }
        }
        stuck.reverse();
        self.held = stuck;
        self.held.len()
    }
}

impl<S: InputSink> Drop for Executor<S> {
    fn drop(&mut self) {
        self.release_all();
    }
}

#[cfg(windows)]
pub use win::SendInputSink;

#[cfg(windows)]
mod win {
    use windows::Win32::UI::Input::KeyboardAndMouse::*;

    use super::{InputSink, Key, MouseButton, SynthEvent, YUBI_INJECT_MARK};

    /// Real sink: `SendInput` tagged with [`YUBI_INJECT_MARK`].
    #[derive(Default)]
    pub struct SendInputSink;

    fn is_extended(vk: u16) -> bool {
        matches!(
            VIRTUAL_KEY(vk),
            VK_LEFT
                | VK_RIGHT
                | VK_UP
                | VK_DOWN
                | VK_HOME
                | VK_END
                | VK_PRIOR
                | VK_NEXT
                | VK_INSERT
                | VK_DELETE
                | VK_RCONTROL
                | VK_RMENU
                | VK_LWIN
                | VK_RWIN
        )
    }

    fn wheel_input(flags: MOUSE_EVENT_FLAGS, amount: i32) -> INPUT {
        INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: 0,
                    dy: 0,
                    // The field is unsigned; a negative amount is its two's complement.
                    mouseData: amount as u32,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: YUBI_INJECT_MARK,
                },
            },
        }
    }

    /// The wheel events of one `Wheel(dx, dy)` (horizontal first; zero axes are skipped).
    fn wheel_inputs(dx: i32, dy: i32) -> Vec<INPUT> {
        let mut v = Vec::with_capacity(2);
        if dx != 0 {
            v.push(wheel_input(MOUSEEVENTF_HWHEEL, dx));
        }
        if dy != 0 {
            v.push(wheel_input(MOUSEEVENTF_WHEEL, dy));
        }
        v
    }

    fn to_input(ev: SynthEvent) -> INPUT {
        let (key, up) = match ev {
            SynthEvent::Wheel(..) => unreachable!("wheel events go through wheel_inputs"),
            SynthEvent::Down(k) => (k, false),
            SynthEvent::Up(k) => (k, true),
            SynthEvent::Move(dx, dy) => {
                return INPUT {
                    r#type: INPUT_MOUSE,
                    Anonymous: INPUT_0 {
                        mi: MOUSEINPUT {
                            dx,
                            dy,
                            mouseData: 0,
                            dwFlags: MOUSEEVENTF_MOVE,
                            time: 0,
                            dwExtraInfo: YUBI_INJECT_MARK,
                        },
                    },
                };
            }
        };
        match key {
            Key::Vk(vk) => {
                let mut flags = KEYBD_EVENT_FLAGS(0);
                if up {
                    flags |= KEYEVENTF_KEYUP;
                }
                if is_extended(vk) {
                    flags |= KEYEVENTF_EXTENDEDKEY;
                }
                INPUT {
                    r#type: INPUT_KEYBOARD,
                    Anonymous: INPUT_0 {
                        ki: KEYBDINPUT {
                            wVk: VIRTUAL_KEY(vk),
                            wScan: 0,
                            dwFlags: flags,
                            time: 0,
                            dwExtraInfo: YUBI_INJECT_MARK,
                        },
                    },
                }
            }
            Key::Mouse(b) => {
                let flags = match (b, up) {
                    (MouseButton::Left, false) => MOUSEEVENTF_LEFTDOWN,
                    (MouseButton::Left, true) => MOUSEEVENTF_LEFTUP,
                    (MouseButton::Right, false) => MOUSEEVENTF_RIGHTDOWN,
                    (MouseButton::Right, true) => MOUSEEVENTF_RIGHTUP,
                    (MouseButton::Middle, false) => MOUSEEVENTF_MIDDLEDOWN,
                    (MouseButton::Middle, true) => MOUSEEVENTF_MIDDLEUP,
                };
                INPUT {
                    r#type: INPUT_MOUSE,
                    Anonymous: INPUT_0 {
                        mi: MOUSEINPUT {
                            dx: 0,
                            dy: 0,
                            mouseData: 0,
                            dwFlags: flags,
                            time: 0,
                            dwExtraInfo: YUBI_INJECT_MARK,
                        },
                    },
                }
            }
        }
    }

    impl InputSink for SendInputSink {
        fn send(&mut self, ev: SynthEvent) -> bool {
            let inputs = match ev {
                SynthEvent::Wheel(dx, dy) => wheel_inputs(dx, dy),
                other => vec![to_input(other)],
            };
            if inputs.is_empty() {
                return true;
            }
            unsafe { SendInput(&inputs, size_of::<INPUT>() as i32) == inputs.len() as u32 }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn events_carry_the_inject_mark() {
            for ev in [
                SynthEvent::Down(Key::Vk(0x41)),
                SynthEvent::Up(Key::Mouse(MouseButton::Left)),
            ] {
                let i = to_input(ev);
                let extra = unsafe {
                    if i.r#type == INPUT_KEYBOARD {
                        i.Anonymous.ki.dwExtraInfo
                    } else {
                        i.Anonymous.mi.dwExtraInfo
                    }
                };
                assert_eq!(extra, YUBI_INJECT_MARK);
            }
        }

        #[test]
        fn wheel_events_carry_amount_flags_and_mark() {
            let v = wheel_inputs(-60, 240);
            assert_eq!(v.len(), 2);
            let (h, w) = unsafe { (v[0].Anonymous.mi, v[1].Anonymous.mi) };
            assert!(h.dwFlags.contains(MOUSEEVENTF_HWHEEL));
            assert_eq!(h.mouseData as i32, -60);
            assert!(w.dwFlags.contains(MOUSEEVENTF_WHEEL));
            assert_eq!(w.mouseData as i32, 240);
            assert_eq!(
                (h.dwExtraInfo, w.dwExtraInfo),
                (YUBI_INJECT_MARK, YUBI_INJECT_MARK)
            );
            assert_eq!(wheel_inputs(0, 0).len(), 0);
        }

        #[test]
        fn arrow_keys_are_extended() {
            let i = to_input(SynthEvent::Down(Key::Vk(VK_LEFT.0)));
            assert!(unsafe { i.Anonymous.ki.dwFlags }.contains(KEYEVENTF_EXTENDEDKEY));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Fake {
        log: Vec<SynthEvent>,
        refuse_up: Vec<Key>,
        refuse_down: bool,
    }

    impl InputSink for &mut Fake {
        fn send(&mut self, ev: SynthEvent) -> bool {
            match ev {
                SynthEvent::Down(_) if self.refuse_down => return false,
                SynthEvent::Up(k) if self.refuse_up.contains(&k) => return false,
                _ => {}
            }
            self.log.push(ev);
            true
        }
    }

    const ALT: Key = Key::Vk(0x12);
    const TAB: Key = Key::Vk(0x09);
    const LMB: Key = Key::Mouse(MouseButton::Left);

    #[test]
    fn press_is_idempotent_and_tracked() {
        let mut f = Fake::default();
        let mut e = Executor::new(&mut f);
        assert!(e.press(ALT));
        assert!(e.press(ALT));
        assert_eq!(e.held(), &[ALT]);
        drop(e);
        // Dropping releases.
        assert_eq!(f.log, vec![SynthEvent::Down(ALT), SynthEvent::Up(ALT)]);
    }

    #[test]
    fn release_only_what_is_held() {
        let mut f = Fake::default();
        let mut e = Executor::new(&mut f);
        e.press(ALT);
        assert!(e.release(TAB)); // never pressed: nothing sent
        assert!(e.release(ALT));
        assert!(e.held().is_empty());
        drop(e);
        assert_eq!(f.log, vec![SynthEvent::Down(ALT), SynthEvent::Up(ALT)]);
    }

    #[test]
    fn release_all_goes_in_reverse_order() {
        let mut f = Fake::default();
        let mut e = Executor::new(&mut f);
        e.press(ALT);
        e.press(TAB);
        e.press(LMB);
        assert_eq!(e.release_all(), 0);
        assert!(e.held().is_empty());
        assert_eq!(e.release_all(), 0);
        drop(e);
        assert_eq!(
            f.log[3..],
            [
                SynthEvent::Up(LMB),
                SynthEvent::Up(TAB),
                SynthEvent::Up(ALT)
            ]
        );
    }

    #[test]
    fn tap_presses_in_order_and_releases_in_reverse() {
        let mut f = Fake::default();
        let mut e = Executor::new(&mut f);
        assert!(e.tap(&[ALT, TAB]));
        assert!(e.held().is_empty());
        drop(e);
        assert_eq!(
            f.log,
            vec![
                SynthEvent::Down(ALT),
                SynthEvent::Down(TAB),
                SynthEvent::Up(TAB),
                SynthEvent::Up(ALT)
            ]
        );
    }

    #[test]
    fn tap_leaves_already_held_keys_alone() {
        let mut f = Fake::default();
        let mut e = Executor::new(&mut f);
        e.press(ALT);
        assert!(e.tap(&[ALT, TAB]));
        assert_eq!(e.held(), &[ALT]);
    }

    #[test]
    fn tap_lets_go_when_a_press_is_refused() {
        struct Picky(Vec<SynthEvent>);
        impl InputSink for &mut Picky {
            fn send(&mut self, ev: SynthEvent) -> bool {
                if ev == SynthEvent::Down(TAB) {
                    return false;
                }
                self.0.push(ev);
                true
            }
        }
        let mut f = Picky(Vec::new());
        let mut e = Executor::new(&mut f);
        assert!(!e.tap(&[ALT, TAB]));
        assert!(e.held().is_empty());
        drop(e);
        assert_eq!(f.0, vec![SynthEvent::Down(ALT), SynthEvent::Up(ALT)]);
    }

    #[test]
    fn move_mouse_skips_zero_and_sends_otherwise() {
        let mut f = Fake::default();
        let mut e = Executor::new(&mut f);
        assert!(e.move_mouse(0, 0));
        assert!(e.move_mouse(3, -2));
        drop(e);
        assert_eq!(f.log, vec![SynthEvent::Move(3, -2)]);
    }

    #[test]
    fn refused_down_is_not_owned() {
        let mut f = Fake {
            refuse_down: true,
            ..Fake::default()
        };
        let mut e = Executor::new(&mut f);
        assert!(!e.press(ALT));
        assert!(e.held().is_empty());
    }

    #[test]
    fn refused_up_stays_owned_for_retry() {
        let mut f = Fake {
            refuse_up: vec![TAB],
            ..Fake::default()
        };
        let mut e = Executor::new(&mut f);
        e.press(ALT);
        e.press(TAB);
        assert_eq!(e.release_all(), 1);
        assert_eq!(e.held(), &[TAB]);
        assert!(!e.release(TAB));
        assert_eq!(e.held(), &[TAB]);
    }
}
