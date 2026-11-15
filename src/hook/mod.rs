//! `WH_MOUSE_LL` suppression: freeze the cursor and/or hold clicks while a finger is on the
//! trackpad (M3b). Proven on hardware by `src/bin/hook-probe.rs`; see `docs/hook-probe.md`.
//!
//! * [`Hook::start`] runs a dedicated thread that owns the hook and its message loop.
//! * [`Hook::set_mode`] / [`Hook::set_hold_buttons`] are lock-free atomics, callable from any
//!   thread. Per hardware tests, set a freeze mode as soon as touch-down Raw Input arrives
//!   (about 67 ms before the first cursor move) and clear it when the fingers lift.
//! * The callback only touches atomics and its thread-local state, calls [`decide`],
//!   `try_send`s and returns (constant work, no allocation, no logging; `try_send` on a
//!   bounded std channel is the only non-atomic synchronisation).
//! * [`Hook::held_buttons`] is the authoritative held state; events may be dropped when the
//!   queue is full (counted in [`HookStats::dropped_events`]).
//! * yubi's own `SendInput` events (`dwExtraInfo == YUBI_INJECT_MARK`) and all other injected
//!   events always pass. The hook cannot tell a USB mouse from the trackpad: both freeze.
//!
//! Lease: every non-`Off` mode expires `lease_ms` (default 250) after the last [`Hook::renew`]
//! (or `set_mode` to a non-`Off` mode, which renews). The engine calls `renew` on every fresh
//! input frame while it needs suppression, so a dead engine releases the mouse by itself. On
//! expiry the hook treats the mode as `Off` (held ups are still swallowed), switches the mode
//! to `Off` and emits [`HookEvent::LeaseExpired`] once.
//!
//! Emergency release: a `WH_KEYBOARD_LL` hook on the same thread watches Ctrl+Alt+Esc (change
//! with [`Hook::set_emergency`]). On the combo the mode goes `Off`, the watchdog latch is set
//! (freeze modes stay refused until `set_mode(Off)`), [`HookEvent::Emergency`] is emitted and
//! the key event itself still passes.
//!
//! Expiry (and the emergency release) takes effect on the next input event: until then
//! [`Hook::mode`] may still report the old mode. All shared atomics use `SeqCst` (a superset of
//! the Release/Acquire pairs needed between `set_mode`, `renew` and the callbacks).
//!
//! Health: Windows silently removes a hook whose callback exceeds `LowLevelHooksTimeout`.
//! [`Hook::last_seen`] reports the age of the last callback; the engine should call
//! [`Hook::reinstall`] when Raw Input shows pointer motion but `last_seen()` is older than
//! about 2 s. A freeze that stays set longer than `max_freeze_ms` (default 10 s) is cancelled
//! by the hook itself, which emits [`HookEvent::Watchdog`], so a stuck flag cannot lock the mouse.
//!
//! Wiring into the engine comes later, hence `allow(dead_code)`.
#![allow(dead_code)]

mod core;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU8, AtomicU32, AtomicU64, Ordering::SeqCst};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::time::Duration;

use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::*;

#[allow(unused_imports)]
pub use self::core::{
    Button, DEFAULT_LEASE_MS, DEFAULT_MAX_FREEZE_MS, EmergencyCombo, HookState, KbState, LlEvent,
    LlKind, MOD_ALT, MOD_CTRL, MOD_SHIFT, MOD_WIN, Mode, VK_ESCAPE, Verdict, decide,
};

/// Posted to the hook thread to unhook and hook again.
const WM_REINSTALL: u32 = WM_APP + 1;
const LLMHF_INJECTED: u32 = 0x01;
const LLMHF_LOWER_IL_INJECTED: u32 = 0x02;
const EVENT_QUEUE: usize = 256;

/// Events from the hook thread. Timestamps are monotonic microseconds ([`now_us`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HookEvent {
    /// A button down was swallowed.
    ButtonHeld { button: Button, t_us: i64 },
    /// The up of a held down was swallowed.
    ButtonReleased { button: Button, t_us: i64 },
    /// `FreezeCursor` outlived `max_freeze_ms`; the hook switched itself to `Off`.
    Watchdog { t_us: i64 },
    /// The lease ran out (no `renew` for `lease_ms`); the hook switched itself to `Off`.
    LeaseExpired { t_us: i64 },
    /// The emergency key combination was pressed; the hook switched itself to `Off`.
    Emergency { t_us: i64 },
}

/// Counter snapshot.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HookStats {
    pub swallowed_moves: u64,
    pub swallowed_buttons: u64,
    pub passed: u64,
    /// Events dropped because the bounded event queue was full.
    pub dropped_events: u64,
    pub reinstalls: u64,
    pub reinstall_failures: u64,
}

#[derive(Default)]
struct Shared {
    mode: AtomicU8,
    hold_mask: AtomicU8,
    /// Bumped by `set_mode` on a real mode change so the callback restarts its watchdog window.
    epoch: AtomicU32,
    /// Set when the watchdog fired; cleared by `set_mode(Off)`.
    watchdog_latched: AtomicBool,
    /// QPC microseconds of the last `touch_alive`; 0 = never.
    alive_us: AtomicI64,
    /// Mirror of `HookState::held` for the engine.
    held: AtomicU8,
    installed: AtomicBool,
    reinstall_failures: AtomicU64,
    max_freeze_ms: AtomicU64,
    /// QPC microseconds of the last `renew` / non-Off `set_mode`.
    last_renew_us: AtomicI64,
    lease_ms: AtomicU64,
    emergency_vk: AtomicU32,
    emergency_mods: AtomicU8,
    /// QPC microseconds of the last callback; 0 = never.
    last_seen_us: AtomicI64,
    stop: AtomicBool,
    swallowed_moves: AtomicU64,
    swallowed_buttons: AtomicU64,
    passed: AtomicU64,
    dropped: AtomicU64,
    reinstalls: AtomicU64,
}

/// Monotonic microseconds (QueryPerformanceCounter).
pub fn now_us() -> i64 {
    use std::sync::OnceLock;
    static FREQ: OnceLock<i64> = OnceLock::new();
    let freq = *FREQ.get_or_init(|| {
        let mut f = 0i64;
        unsafe {
            let _ = QueryPerformanceFrequency(&mut f);
        }
        f.max(1)
    });
    let mut c = 0i64;
    unsafe {
        let _ = QueryPerformanceCounter(&mut c);
    }
    (c as i128 * 1_000_000 / freq as i128) as i64
}

/// Everything the callback needs; lives in a thread-local of the hook thread (WH_MOUSE_LL
/// callbacks run on the thread that installed the hook).
struct Ctx {
    shared: Arc<Shared>,
    tx: SyncSender<HookEvent>,
    state: HookState,
    kb: KbState,
    epoch: u32,
}

thread_local! {
    static CTX: std::cell::RefCell<Option<Ctx>> = const { std::cell::RefCell::new(None) };
}

fn button_msg(msg: u32, mouse_data: u32) -> LlKind {
    // XBUTTON1 = 1, XBUTTON2 = 2 in the high word of `mouseData`.
    let x = if (mouse_data >> 16) as u16 == 2 {
        Button::X2
    } else {
        Button::X1
    };
    match msg {
        WM_MOUSEMOVE => LlKind::Move,
        WM_LBUTTONDOWN => LlKind::Down(Button::Left),
        WM_LBUTTONUP => LlKind::Up(Button::Left),
        WM_RBUTTONDOWN => LlKind::Down(Button::Right),
        WM_RBUTTONUP => LlKind::Up(Button::Right),
        WM_MBUTTONDOWN => LlKind::Down(Button::Middle),
        WM_MBUTTONUP => LlKind::Up(Button::Middle),
        WM_XBUTTONDOWN => LlKind::Down(x),
        WM_XBUTTONUP => LlKind::Up(x),
        _ => LlKind::Other,
    }
}

unsafe extern "system" fn ll_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code < 0 {
        return unsafe { CallNextHookEx(None, code, wparam, lparam) };
    }
    let t_us = now_us();
    let ms = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
    let ev = LlEvent {
        kind: button_msg(wparam.0 as u32, ms.mouseData),
        injected: ms.flags & (LLMHF_INJECTED | LLMHF_LOWER_IL_INJECTED) != 0,
        extra: ms.dwExtraInfo,
        t_us,
    };
    let verdict = CTX.with(|c| {
        // Re-entrancy cannot happen, but never panic inside a hook: pass on any doubt.
        let Ok(mut guard) = c.try_borrow_mut() else {
            return Verdict::Pass;
        };
        let Some(ctx) = guard.as_mut() else {
            return Verdict::Pass;
        };
        let sh = &ctx.shared;
        sh.last_seen_us.store(t_us, SeqCst);
        let epoch = sh.epoch.load(SeqCst);
        if epoch != ctx.epoch {
            ctx.epoch = epoch;
            ctx.state.reset_freeze();
        }
        ctx.state.hold_mask = sh.hold_mask.load(SeqCst);
        ctx.state.last_alive_us = match sh.alive_us.load(SeqCst) {
            0 => i64::MIN,
            t => t,
        };
        ctx.state.max_freeze_us = (sh.max_freeze_ms.load(SeqCst).min(1 << 40) * 1000) as i64;
        let mode = Mode::from_u8(sh.mode.load(SeqCst));
        let lease_us = (sh.lease_ms.load(SeqCst).min(1 << 40) * 1000) as i64;
        let lease_until = sh.last_renew_us.load(SeqCst).saturating_add(lease_us);
        let v = decide(&mut ctx.state, mode, lease_until, ev);
        sh.held.store(ctx.state.held, SeqCst);
        if ctx.state.lease_expired {
            // Re-read the lease right before the CAS: a `renew` / re-assert of the same mode
            // since we judged it keeps the mode alive.
            let renewed = sh.last_renew_us.load(SeqCst).saturating_add(lease_us) >= t_us;
            if !renewed
                && sh
                    .mode
                    .compare_exchange(mode.to_u8(), Mode::Off.to_u8(), SeqCst, SeqCst)
                    .is_ok()
            {
                emit(ctx, HookEvent::LeaseExpired { t_us });
            }
            ctx.state.ack_lease();
        }
        if ctx.state.watchdog_tripped {
            // Latch first so a concurrent re-assert of a freeze mode is downgraded.
            sh.watchdog_latched.store(true, SeqCst);
            // Only cancel the mode we judged; a newer `set_mode` wins.
            let _ = sh
                .mode
                .compare_exchange(mode.to_u8(), Mode::Off.to_u8(), SeqCst, SeqCst);
            ctx.state.ack_watchdog();
            emit(ctx, HookEvent::Watchdog { t_us });
        }
        let sh = &ctx.shared;
        match v {
            Verdict::Pass => {
                sh.passed.fetch_add(1, SeqCst);
            }
            Verdict::Swallow => {
                sh.swallowed_moves.fetch_add(1, SeqCst);
            }
            Verdict::SwallowAndReport => {
                sh.swallowed_buttons.fetch_add(1, SeqCst);
                let out = match ev.kind {
                    LlKind::Down(button) => Some(HookEvent::ButtonHeld { button, t_us }),
                    LlKind::Up(button) => Some(HookEvent::ButtonReleased { button, t_us }),
                    _ => None,
                };
                if let Some(out) = out {
                    emit(ctx, out);
                }
            }
        }
        v
    });
    if verdict == Verdict::Pass {
        unsafe { CallNextHookEx(None, code, wparam, lparam) }
    } else {
        LRESULT(1)
    }
}

/// Cross-check the tracked modifiers with the OS so stale bits (missed ups while unhooked or
/// on the secure desktop) never trigger the emergency release. No allocation, callback-safe.
fn modifiers_really_down(mods: u8) -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
    let down = |vk: i32| unsafe { GetAsyncKeyState(vk) } < 0;
    (mods & MOD_CTRL == 0 || down(0x11))
        && (mods & MOD_ALT == 0 || down(0x12))
        && (mods & MOD_SHIFT == 0 || down(0x10))
        && (mods & MOD_WIN == 0 || down(0x5B) || down(0x5C))
}

/// `WH_KEYBOARD_LL` callback: watches the emergency combo. Never swallows a key.
unsafe extern "system" fn kb_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        let kb = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        let down = match wparam.0 as u32 {
            WM_KEYDOWN | WM_SYSKEYDOWN => Some(true),
            WM_KEYUP | WM_SYSKEYUP => Some(false),
            _ => None,
        };
        if let Some(down) = down {
            CTX.with(|c| {
                let Ok(mut guard) = c.try_borrow_mut() else {
                    return;
                };
                let Some(ctx) = guard.as_mut() else {
                    return;
                };
                let sh = &ctx.shared;
                let combo = EmergencyCombo {
                    vk: sh.emergency_vk.load(SeqCst),
                    mods: sh.emergency_mods.load(SeqCst),
                };
                if ctx.kb.on_key(combo, kb.vkCode, down) && modifiers_really_down(combo.mods) {
                    // Latch first so a concurrent re-assert of a freeze mode is downgraded.
                    sh.watchdog_latched.store(true, SeqCst);
                    let prev = sh.mode.swap(Mode::Off.to_u8(), SeqCst);
                    if prev != Mode::Off.to_u8() {
                        sh.epoch.fetch_add(1, SeqCst);
                    }
                    emit(ctx, HookEvent::Emergency { t_us: now_us() });
                }
            });
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

fn emit(ctx: &Ctx, ev: HookEvent) {
    if ctx.tx.try_send(ev).is_err() {
        ctx.shared.dropped.fetch_add(1, SeqCst);
    }
}

/// The mouse hook and the emergency keyboard hook, installed and removed together.
struct Hooks {
    mouse: HHOOK,
    keyboard: HHOOK,
}

impl Hooks {
    fn uninstall(self) {
        unsafe {
            let _ = UnhookWindowsHookEx(self.mouse);
            let _ = UnhookWindowsHookEx(self.keyboard);
        }
    }
}

fn install() -> windows::core::Result<Hooks> {
    unsafe {
        let hinstance: windows::Win32::Foundation::HINSTANCE = GetModuleHandleW(None)?.into();
        let mouse = SetWindowsHookExW(WH_MOUSE_LL, Some(ll_proc), Some(hinstance), 0)?;
        match SetWindowsHookExW(WH_KEYBOARD_LL, Some(kb_proc), Some(hinstance), 0) {
            Ok(keyboard) => Ok(Hooks { mouse, keyboard }),
            Err(e) => {
                let _ = UnhookWindowsHookEx(mouse);
                Err(e)
            }
        }
    }
}

/// Handle to the running hook thread. Dropping it stops the hook.
pub struct Hook {
    shared: Arc<Shared>,
    rx: Receiver<HookEvent>,
    thread_id: u32,
    join: Option<std::thread::JoinHandle<()>>,
}

impl Hook {
    /// Start the hook thread with mode `Off`, all buttons in the hold set and the default
    /// freeze limit.
    pub fn start() -> windows::core::Result<Hook> {
        let shared = Arc::new(Shared::default());
        shared.hold_mask.store(Button::ALL, SeqCst);
        shared.max_freeze_ms.store(DEFAULT_MAX_FREEZE_MS, SeqCst);
        shared.lease_ms.store(DEFAULT_LEASE_MS, SeqCst);
        let combo = EmergencyCombo::default();
        shared.emergency_vk.store(combo.vk, SeqCst);
        shared.emergency_mods.store(combo.mods, SeqCst);
        let (tx, rx) = sync_channel(EVENT_QUEUE);
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<windows::core::Result<u32>>();
        let sh = shared.clone();
        let join = std::thread::Builder::new()
            .name("yubi-mouse-hook".into())
            .spawn(move || hook_thread(sh, tx, ready_tx))
            .map_err(|e| windows::core::Error::new(windows::core::HRESULT(-1), e.to_string()))?;
        let id = match ready_rx.recv() {
            Ok(Ok(id)) => id,
            Ok(Err(e)) => {
                let _ = join.join();
                return Err(e);
            }
            Err(_) => {
                let _ = join.join();
                return Err(windows::core::Error::new(
                    windows::core::HRESULT(-1),
                    "hook thread exited before start",
                ));
            }
        };
        let hook = Hook {
            shared,
            rx,
            thread_id: id,
            join: Some(join),
        };
        // `stop` can only be requested through this handle, so no stop can precede the id;
        // the thread's queue already exists, so a posted WM_QUIT is never lost.
        Ok(hook)
    }

    /// Switch what is suppressed. Lock-free, callable from any thread.
    ///
    /// Re-asserting the current mode is a no-op (it does not restart the watchdog window).
    /// After a [`HookEvent::Watchdog`], freeze modes are refused (downgraded to `Off` /
    /// `HoldClicks`) until the caller sets `Off` once, e.g. when the fingers lift.
    /// A non-`Off` mode also renews the lease (before the mode is published, so the callback
    /// never sees a new mode with an old lease).
    pub fn set_mode(&self, mode: Mode) {
        let sh = &self.shared;
        if mode != Mode::Off {
            sh.last_renew_us.store(now_us(), SeqCst);
        }
        if mode == Mode::Off {
            sh.watchdog_latched.store(false, SeqCst);
        }
        let mut mode = mode;
        if mode.freezes() && sh.watchdog_latched.load(SeqCst) {
            mode = mode.without_freeze();
        }
        let prev = sh.mode.swap(mode.to_u8(), SeqCst);
        // A watchdog / emergency may have latched between the check and the swap: re-check
        // after publishing and downgrade (only if our mode is still the published one).
        if mode.freezes() && sh.watchdog_latched.load(SeqCst) {
            let down = mode.without_freeze();
            if sh
                .mode
                .compare_exchange(mode.to_u8(), down.to_u8(), SeqCst, SeqCst)
                .is_ok()
            {
                mode = down;
            }
        }
        if prev != mode.to_u8() {
            sh.epoch.fetch_add(1, SeqCst);
        }
    }

    /// Keep the current suppression mode alive for another `lease_ms`. Lock-free; call it on
    /// every fresh input frame while suppression is needed.
    pub fn renew(&self) {
        self.shared.last_renew_us.store(now_us(), SeqCst);
    }

    /// The engine saw a finger on the pad just now: restart the watchdog's idle window (the
    /// absolute cap still applies). Lock-free; call it with `renew` while a live touch holds
    /// the freeze. Without it a freeze still trips after `max_freeze_ms`.
    pub fn touch_alive(&self) {
        self.shared.alive_us.store(now_us().max(1), SeqCst);
    }

    /// True from the moment the watchdog fired (or the emergency combo) until `set_mode(Off)`.
    pub fn watchdog_tripped(&self) -> bool {
        self.shared.watchdog_latched.load(SeqCst)
    }

    /// Lease length (default [`DEFAULT_LEASE_MS`]).
    pub fn set_lease_ms(&self, ms: u64) {
        self.shared.lease_ms.store(ms, SeqCst);
    }

    /// Emergency key combination: virtual-key code and `MOD_*` bits (default Esc, Ctrl+Alt).
    pub fn set_emergency(&self, vk: u32, mods: u8) {
        self.shared.emergency_mods.store(mods, SeqCst);
        self.shared.emergency_vk.store(vk, SeqCst);
    }

    /// Buttons whose down was swallowed and whose up has not been seen yet (bits of
    /// [`Button::bit`]). Authoritative even if a `ButtonReleased` event was dropped.
    pub fn held_buttons(&self) -> u8 {
        self.shared.held.load(SeqCst)
    }

    /// Whether the hook is currently installed (false after a failed [`Hook::reinstall`]).
    pub fn installed(&self) -> bool {
        self.shared.installed.load(SeqCst)
    }

    pub fn mode(&self) -> Mode {
        Mode::from_u8(self.shared.mode.load(SeqCst))
    }

    /// Which buttons `HoldClicks` swallows (bit mask of [`Button::bit`]; default all).
    pub fn set_hold_buttons(&self, mask: u8) {
        self.shared.hold_mask.store(mask & Button::ALL, SeqCst);
    }

    /// Limit for a continuous freeze before the self-cancelling watchdog fires.
    pub fn set_max_freeze_ms(&self, ms: u64) {
        self.shared.max_freeze_ms.store(ms, SeqCst);
    }

    /// Events from the hook (held / released buttons, watchdog).
    pub fn events(&self) -> &Receiver<HookEvent> {
        &self.rx
    }

    /// Drain pending events without blocking.
    pub fn drain_events(&self) -> Vec<HookEvent> {
        let mut v = Vec::new();
        while let Ok(e) = self.rx.try_recv() {
            v.push(e);
        }
        v
    }

    pub fn stats(&self) -> HookStats {
        let s = &self.shared;
        HookStats {
            swallowed_moves: s.swallowed_moves.load(SeqCst),
            swallowed_buttons: s.swallowed_buttons.load(SeqCst),
            passed: s.passed.load(SeqCst),
            dropped_events: s.dropped.load(SeqCst),
            reinstalls: s.reinstalls.load(SeqCst),
            reinstall_failures: s.reinstall_failures.load(SeqCst),
        }
    }

    /// Age of the last callback, or `None` if the hook never saw an event.
    pub fn last_seen(&self) -> Option<Duration> {
        let t = self.shared.last_seen_us.load(SeqCst);
        (t != 0).then(|| Duration::from_micros((now_us() - t).max(0) as u64))
    }

    /// Unhook and hook again on the hook thread (asynchronous). Use when Raw Input shows motion
    /// but [`Hook::last_seen`] is older than about 2 s (Windows removed a slow hook).
    pub fn reinstall(&self) {
        unsafe {
            let _ = PostThreadMessageW(self.thread_id, WM_REINSTALL, WPARAM(0), LPARAM(0));
        }
    }

    /// Unhook, end the message loop and join the thread. Idempotent.
    pub fn stop(&mut self) {
        self.shared.stop.store(true, SeqCst);
        if let Some(join) = self.join.take() {
            unsafe {
                let _ = PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
            }
            let _ = join.join();
        }
    }

    /// Drop pending events (e.g. after a mode change you do not care about old ones).
    /// Do not call while a button is held unless you track [`Hook::held_buttons`] instead:
    /// a dropped `ButtonHeld` would orphan its later `ButtonReleased`.
    pub fn clear_events(&self) {
        while self.rx.try_recv().is_ok() {}
    }
}

impl Drop for Hook {
    fn drop(&mut self) {
        self.stop();
    }
}

fn hook_thread(
    shared: Arc<Shared>,
    tx: SyncSender<HookEvent>,
    ready: std::sync::mpsc::Sender<windows::core::Result<u32>>,
) {
    unsafe {
        // Force the thread message queue to exist before the id is published, so a
        // `PostThreadMessageW` from `stop`/`reinstall` can never be lost.
        let mut msg = MSG::default();
        let _ = PeekMessageW(&mut msg, None, WM_USER, WM_USER, PM_NOREMOVE);
        let mut hook = match install() {
            Ok(h) => Some(h),
            Err(e) => {
                let _ = ready.send(Err(e));
                return;
            }
        };
        CTX.with(|c| {
            *c.borrow_mut() = Some(Ctx {
                state: HookState::new(
                    shared.hold_mask.load(SeqCst),
                    shared.max_freeze_ms.load(SeqCst),
                ),
                kb: KbState::default(),
                epoch: shared.epoch.load(SeqCst),
                shared: shared.clone(),
                tx,
            });
        });
        shared.installed.store(true, SeqCst);
        let _ = ready.send(Ok(GetCurrentThreadId()));
        // Stop requested before the loop started: the WM_QUIT is queued anyway, but do not
        // rely on it.
        while !shared.stop.load(SeqCst) && GetMessageW(&mut msg, None, 0, 0).0 > 0 {
            if msg.message == WM_REINSTALL {
                if let Some(h) = hook.take() {
                    h.uninstall();
                }
                // An up that arrives while unhooked is never seen. Release every held button
                // now so the engine cannot keep a stuck hold (a late physical up then reaches
                // the OS as a harmless unmatched up).
                CTX.with(|c| {
                    if let Ok(mut g) = c.try_borrow_mut()
                        && let Some(ctx) = g.as_mut()
                    {
                        let t_us = now_us();
                        for b in Button::EACH {
                            if ctx.state.held & b.bit() != 0 {
                                emit(ctx, HookEvent::ButtonReleased { button: b, t_us });
                            }
                        }
                        ctx.state.held = 0;
                        ctx.kb = KbState::default();
                        ctx.shared.held.store(0, SeqCst);
                    }
                });
                match install() {
                    Ok(h) => {
                        hook = Some(h);
                        shared.reinstalls.fetch_add(1, SeqCst);
                    }
                    Err(_) => {
                        shared.reinstall_failures.fetch_add(1, SeqCst);
                    }
                }
                shared.installed.store(hook.is_some(), SeqCst);
                continue;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        if let Some(h) = hook.take() {
            h.uninstall();
        }
        shared.installed.store(false, SeqCst);
        shared.held.store(0, SeqCst);
        CTX.with(|c| *c.borrow_mut() = None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Starts and stops the real hook. Needs an interactive desktop session.
    #[test]
    #[ignore = "installs a real WH_MOUSE_LL hook"]
    fn start_and_stop() {
        let mut h = Hook::start().expect("hook starts");
        assert_eq!(h.mode(), Mode::Off);
        h.set_mode(Mode::FreezeCursor);
        h.set_mode(Mode::Off);
        h.reinstall();
        h.stop();
        h.stop();
    }

    #[test]
    fn now_us_is_monotonic() {
        let a = now_us();
        let b = now_us();
        assert!(b >= a);
    }
}
