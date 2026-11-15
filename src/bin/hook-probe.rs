//! M3 gate probe: can a WH_MOUSE_LL hook suppress trackpad pointer input, and how do
//! Raw Input and the hook interleave?
//!
//! Standalone dev tool (no egui, no dependency on `src/input`). One thread owns a
//! message-only window that receives Raw Input for the PTP touch pad (0x0D/0x05) and
//! for mice (0x01/0x02) with `RIDEV_INPUTSINK`, and the same thread installs the
//! `WH_MOUSE_LL` hook, so hook callbacks and `WM_INPUT` are serialised on one queue.
//! The hook only reads atomics and pushes a small event onto a channel; a logger thread
//! prints lines and computes the summary.
//!
//! Usage: `hook-probe [--suppress] [--seconds N]` (default 20 s).
//!
//! "contacts" is derived from Tip Switch flags across the hybrid reports of one frame
//! (the first report announces Contact Count, following reports carry the remaining
//! fingers). This is an approximation that is good enough for an "any finger down" gate.

#[cfg(not(windows))]
fn main() {
    eprintln!("hook-probe is Windows only");
}

#[cfg(windows)]
fn main() {
    win::run();
}

#[cfg(windows)]
mod win {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::io::Write;
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU8, AtomicU32, Ordering::Relaxed};
    use std::sync::mpsc::{Receiver, Sender, channel};

    use windows::Win32::Devices::HumanInterfaceDevice::*;
    use windows::Win32::Foundation::*;
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
    use windows::Win32::UI::Input::*;
    use windows::Win32::UI::WindowsAndMessaging::*;
    use windows::core::{PCWSTR, w};

    const PAGE_DIGITIZER: u16 = 0x0D;
    const USAGE_TOUCH_PAD: u16 = 0x05;
    const USAGE_TIP_SWITCH: u16 = 0x42;
    const USAGE_CONTACT_ID: u16 = 0x51;
    const USAGE_CONTACT_COUNT: u16 = 0x54;
    const LLMHF_INJECTED_BIT: u32 = 0x01;
    /// A move reaching the hook within this window of a Raw Input mouse event is "the same" event.
    const CORRELATION_WINDOW_US: i64 = 2_000;
    /// A move that passed the hook this shortly before a touch-down was processed raced it.
    const PRE_TOUCH_WINDOW_US: i64 = 30_000;

    // Shared state. The hook and the window procedure run on the same thread; the
    // logger thread only receives events.
    static FREQ: OnceLock<i64> = OnceLock::new();
    static TX: OnceLock<Sender<Ev>> = OnceLock::new();
    static TOUCHES: AtomicU32 = AtomicU32::new(0);
    static SUPPRESS: AtomicBool = AtomicBool::new(false);
    static DEADLINE_US: AtomicI64 = AtomicI64::new(i64::MAX);
    /// Bit per button (L, R, M, X) whose Down was swallowed, so its Up is swallowed too.
    static SWALLOWED_DOWN: AtomicU8 = AtomicU8::new(0);

    fn now_us() -> i64 {
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

    fn send(ev: Ev) {
        if let Some(tx) = TX.get() {
            let _ = tx.send(ev);
        }
    }

    enum Ev {
        Info(String),
        Done,
        Ptp {
            ts: i64,
            contacts: u32,
            announced: u32,
        },
        Mouse {
            ts: i64,
            dev: usize,
            trackpad: bool,
            dx: i32,
            dy: i32,
            flags: u16,
        },
        LlMove {
            ts: i64,
            x: i32,
            y: i32,
            flags: u32,
            extra: usize,
            swallowed: bool,
            touching: bool,
        },
        LlButton {
            ts: i64,
            name: &'static str,
            down: bool,
            injected: bool,
            extra: usize,
            swallowed: bool,
        },
    }

    // ---- Raw Input side ----
    struct Ptp {
        preparsed: Vec<u64>,
        fingers: Vec<u16>,
        count_link: Option<u16>,
        expected: u32,
        frame_tips: u32,
        contacts: u32,
    }

    impl Ptp {
        fn pd(&self) -> PHIDP_PREPARSED_DATA {
            PHIDP_PREPARSED_DATA(self.preparsed.as_ptr() as isize)
        }
    }

    enum Kind {
        Ptp(Box<Ptp>),
        Mouse { trackpad: bool },
        Other,
    }

    #[derive(Default)]
    struct Devices {
        map: HashMap<usize, Kind>,
        /// `vid_..&pid_..` keys of the PTP devices (without interface / collection suffix).
        ptp_keys: Vec<String>,
    }

    thread_local! {
        static DEVICES: RefCell<Devices> = RefCell::new(Devices::default());
    }

    /// `\\?\hid#vid_05ac&pid_0324&mi_01&col05#...` -> `vid_05ac&pid_0324`.
    fn device_key(path: &str) -> String {
        let lower = path.to_lowercase();
        let part = lower.split('#').nth(1).unwrap_or(&lower);
        let cut = part
            .find("&mi_")
            .or_else(|| part.find("&col"))
            .unwrap_or(part.len());
        part[..cut].to_string()
    }

    fn device_path(handle: HANDLE) -> Option<String> {
        unsafe {
            let mut len = 0u32;
            GetRawInputDeviceInfoW(Some(handle), RIDI_DEVICENAME, None, &mut len);
            if len == 0 {
                return None;
            }
            let mut buf = vec![0u16; len as usize];
            let got = GetRawInputDeviceInfoW(
                Some(handle),
                RIDI_DEVICENAME,
                Some(buf.as_mut_ptr().cast()),
                &mut len,
            );
            if got == u32::MAX {
                return None;
            }
            while buf.last() == Some(&0) {
                buf.pop();
            }
            Some(String::from_utf16_lossy(&buf))
        }
    }

    fn cap_usages(c: &HIDP_VALUE_CAPS) -> std::ops::RangeInclusive<u16> {
        unsafe {
            if c.IsRange {
                c.Anonymous.Range.UsageMin..=c.Anonymous.Range.UsageMax
            } else {
                let u = c.Anonymous.NotRange.Usage;
                u..=u
            }
        }
    }

    /// Build a PTP decoder for a HID device, or `None` if it is not a touch pad.
    fn build_ptp(handle: HANDLE) -> Option<Ptp> {
        unsafe {
            let mut size = 0u32;
            GetRawInputDeviceInfoW(Some(handle), RIDI_PREPARSEDDATA, None, &mut size);
            if size == 0 {
                return None;
            }
            let mut preparsed = vec![0u64; (size as usize).div_ceil(8)];
            let got = GetRawInputDeviceInfoW(
                Some(handle),
                RIDI_PREPARSEDDATA,
                Some(preparsed.as_mut_ptr().cast()),
                &mut size,
            );
            if got == u32::MAX {
                return None;
            }
            let pd = PHIDP_PREPARSED_DATA(preparsed.as_ptr() as isize);
            let mut caps = HIDP_CAPS::default();
            if HidP_GetCaps(pd, &mut caps) != HIDP_STATUS_SUCCESS
                || caps.UsagePage != PAGE_DIGITIZER
                || caps.Usage != USAGE_TOUCH_PAD
            {
                return None;
            }
            let mut n = caps.NumberInputValueCaps;
            if n == 0 {
                return None;
            }
            let mut vcaps = vec![HIDP_VALUE_CAPS::default(); n as usize];
            if HidP_GetValueCaps(HidP_Input, vcaps.as_mut_ptr(), &mut n, pd) != HIDP_STATUS_SUCCESS
            {
                return None;
            }
            vcaps.truncate(n as usize);
            let mut fingers: Vec<u16> = vcaps
                .iter()
                .filter(|c| {
                    c.UsagePage == PAGE_DIGITIZER
                        && c.LinkCollection != 0
                        && cap_usages(c).contains(&USAGE_CONTACT_ID)
                })
                .map(|c| c.LinkCollection)
                .collect();
            fingers.sort_unstable();
            fingers.dedup();
            if fingers.is_empty() {
                return None;
            }
            let count_link = vcaps
                .iter()
                .find(|c| {
                    c.UsagePage == PAGE_DIGITIZER && cap_usages(c).contains(&USAGE_CONTACT_COUNT)
                })
                .map(|c| c.LinkCollection);
            Some(Ptp {
                preparsed,
                fingers,
                count_link,
                expected: 0,
                frame_tips: 0,
                contacts: 0,
            })
        }
    }

    fn get_value(pd: PHIDP_PREPARSED_DATA, link: u16, usage: u16, report: &[u8]) -> Option<u32> {
        let mut v = 0u32;
        let st = unsafe {
            HidP_GetUsageValue(
                HidP_Input,
                PAGE_DIGITIZER,
                Some(link),
                usage,
                &mut v,
                pd,
                report,
            )
        };
        (st == HIDP_STATUS_SUCCESS).then_some(v)
    }

    /// Tip Switch is a 1-bit field, exposed as a value or as a button depending on the descriptor.
    fn tip_down(pd: PHIDP_PREPARSED_DATA, link: u16, report: &mut [u8]) -> bool {
        if let Some(v) = get_value(pd, link, USAGE_TIP_SWITCH, report) {
            return v != 0;
        }
        let mut buf = [0u16; 16];
        let mut len = buf.len() as u32;
        let st = unsafe {
            HidP_GetUsages(
                HidP_Input,
                PAGE_DIGITIZER,
                Some(link),
                buf.as_mut_ptr(),
                &mut len,
                pd,
                report,
            )
        };
        st == HIDP_STATUS_SUCCESS && buf[..len as usize].contains(&USAGE_TIP_SWITCH)
    }

    /// Fold one hybrid report into the frame state. Returns `(announced, contacts)` for touch reports.
    fn ptp_report(p: &mut Ptp, report: &mut [u8]) -> Option<(u32, u32)> {
        let pd = p.pd();
        get_value(pd, p.fingers[0], USAGE_CONTACT_ID, report)?;
        let announced = p
            .count_link
            .and_then(|l| get_value(pd, l, USAGE_CONTACT_COUNT, report))
            .unwrap_or(0);
        let tips = p
            .fingers
            .iter()
            .filter(|&&l| tip_down(pd, l, report))
            .count() as u32;
        if announced > 0 {
            p.expected = announced;
            p.frame_tips = tips;
        } else if p.frame_tips < p.expected {
            p.frame_tips += tips;
        } else {
            p.expected = 0;
            p.frame_tips = tips;
        }
        p.contacts = p.frame_tips;
        Some((announced, p.contacts))
    }

    fn classify(devs: &mut Devices, handle: HANDLE, dev_type: RID_DEVICE_INFO_TYPE) {
        let key = handle.0 as usize;
        if devs.map.contains_key(&key) {
            return;
        }
        let path = device_path(handle).unwrap_or_default();
        let kind = if dev_type == RIM_TYPEHID {
            match build_ptp(handle) {
                Some(p) => {
                    let k = device_key(&path);
                    send(Ev::Info(format!(
                        "PTP touch pad: handle={key:#x} fingers={} path={path}",
                        p.fingers.len()
                    )));
                    if !devs.ptp_keys.contains(&k) {
                        devs.ptp_keys.push(k);
                    }
                    Kind::Ptp(Box::new(p))
                }
                None => Kind::Other,
            }
        } else if dev_type == RIM_TYPEMOUSE {
            let trackpad = devs.ptp_keys.contains(&device_key(&path));
            send(Ev::Info(format!(
                "mouse: handle={key:#x} trackpad_sibling={trackpad} path={path}"
            )));
            Kind::Mouse { trackpad }
        } else {
            Kind::Other
        };
        devs.map.insert(key, kind);
    }

    unsafe fn enumerate_existing() {
        unsafe {
            let mut count = 0u32;
            let item = size_of::<RAWINPUTDEVICELIST>() as u32;
            if GetRawInputDeviceList(None, &mut count, item) == u32::MAX || count == 0 {
                return;
            }
            let mut list = vec![RAWINPUTDEVICELIST::default(); count as usize];
            let got = GetRawInputDeviceList(Some(list.as_mut_ptr()), &mut count, item);
            if got == u32::MAX {
                return;
            }
            list.truncate(got as usize);
            // PTP pads first, so mice can be matched against them.
            DEVICES.with(|d| {
                let mut d = d.borrow_mut();
                for e in list.iter().filter(|e| e.dwType == RIM_TYPEHID) {
                    classify(&mut d, e.hDevice, RIM_TYPEHID);
                }
                for e in list.iter().filter(|e| e.dwType == RIM_TYPEMOUSE) {
                    classify(&mut d, e.hDevice, RIM_TYPEMOUSE);
                }
            });
        }
    }

    /// Forget a removed device (handles can be reused) and, if it was the touch pad, stop treating
    /// the pad as touched so pointer suppression cannot get stuck on.
    fn device_removed(handle: HANDLE) {
        let removed = DEVICES.with(|d| d.borrow_mut().map.remove(&(handle.0 as usize)));
        if let Some(Kind::Ptp(_)) = removed {
            TOUCHES.store(0, Relaxed);
            send(Ev::Info(format!(
                "PTP touch pad removed: handle={:#x}",
                handle.0 as usize
            )));
        }
    }

    fn handle_input(hraw: HRAWINPUT) {
        unsafe {
            let ts = now_us();
            let mut size = 0u32;
            let header = size_of::<RAWINPUTHEADER>() as u32;
            if GetRawInputData(hraw, RID_INPUT, None, &mut size, header) == u32::MAX || size == 0 {
                return;
            }
            let mut buf = vec![0u64; (size as usize).div_ceil(8)];
            if GetRawInputData(
                hraw,
                RID_INPUT,
                Some(buf.as_mut_ptr().cast()),
                &mut size,
                header,
            ) == u32::MAX
            {
                return;
            }
            let raw = &*(buf.as_ptr().cast::<RAWINPUT>());
            let handle = raw.header.hDevice;
            let dev_type = RID_DEVICE_INFO_TYPE(raw.header.dwType);
            DEVICES.with(|d| {
                let mut d = d.borrow_mut();
                classify(&mut d, handle, dev_type);
                match d.map.get_mut(&(handle.0 as usize)) {
                    Some(Kind::Mouse { trackpad }) => {
                        let m = raw.data.mouse;
                        send(Ev::Mouse {
                            ts,
                            dev: handle.0 as usize,
                            trackpad: *trackpad,
                            dx: m.lLastX,
                            dy: m.lLastY,
                            flags: m.Anonymous.Anonymous.usButtonFlags,
                        });
                    }
                    Some(Kind::Ptp(p)) => {
                        let bytes =
                            std::slice::from_raw_parts(buf.as_ptr().cast::<u8>(), size as usize);
                        let off = header as usize;
                        if bytes.len() < off + 8 {
                            return;
                        }
                        let size_hid =
                            u32::from_ne_bytes(bytes[off..off + 4].try_into().unwrap()) as usize;
                        let count = u32::from_ne_bytes(bytes[off + 4..off + 8].try_into().unwrap())
                            as usize;
                        let data = &bytes[off + 8..];
                        if size_hid == 0 || data.len() < size_hid * count {
                            return;
                        }
                        for i in 0..count {
                            let mut report = data[i * size_hid..(i + 1) * size_hid].to_vec();
                            if let Some((announced, contacts)) = ptp_report(p, &mut report) {
                                TOUCHES.store(contacts, Relaxed);
                                send(Ev::Ptp {
                                    ts,
                                    contacts,
                                    announced,
                                });
                            }
                        }
                    }
                    _ => {}
                }
            });
        }
    }

    unsafe extern "system" fn wndproc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_INPUT => handle_input(HRAWINPUT(lparam.0 as *mut _)),
            WM_INPUT_DEVICE_CHANGE if wparam.0 as u32 == GIDC_REMOVAL => {
                device_removed(HANDLE(lparam.0 as *mut _));
            }
            WM_TIMER if now_us() >= DEADLINE_US.load(Relaxed) => unsafe { PostQuitMessage(0) },
            _ => {}
        }
        unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
    }

    // ---- hook ----
    fn button_of(msg: u32) -> Option<(u8, &'static str, bool)> {
        match msg {
            WM_LBUTTONDOWN => Some((0, "lbutton", true)),
            WM_LBUTTONUP => Some((0, "lbutton", false)),
            WM_RBUTTONDOWN => Some((1, "rbutton", true)),
            WM_RBUTTONUP => Some((1, "rbutton", false)),
            WM_MBUTTONDOWN => Some((2, "mbutton", true)),
            WM_MBUTTONUP => Some((2, "mbutton", false)),
            WM_XBUTTONDOWN => Some((3, "xbutton", true)),
            WM_XBUTTONUP => Some((3, "xbutton", false)),
            _ => None,
        }
    }

    /// Must stay well under 1 ms: atomics, one QPC read, one channel send.
    unsafe extern "system" fn ll_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if code < 0 {
            return unsafe { CallNextHookEx(None, code, wparam, lparam) };
        }
        let ts = now_us();
        let ms = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
        let injected = ms.flags & LLMHF_INJECTED_BIT != 0;
        let touching = TOUCHES.load(Relaxed) > 0;
        let suppress = SUPPRESS.load(Relaxed) && touching && !injected;
        let msg = wparam.0 as u32;
        let mut swallowed = false;
        if msg == WM_MOUSEMOVE {
            swallowed = suppress;
            send(Ev::LlMove {
                ts,
                x: ms.pt.x,
                y: ms.pt.y,
                flags: ms.flags,
                extra: ms.dwExtraInfo,
                swallowed,
                touching,
            });
        } else if let Some((bit, name, down)) = button_of(msg) {
            let mask = 1u8 << bit;
            if down {
                swallowed = suppress;
                if swallowed {
                    SWALLOWED_DOWN.fetch_or(mask, Relaxed);
                } else {
                    SWALLOWED_DOWN.fetch_and(!mask, Relaxed);
                }
            } else if SWALLOWED_DOWN.load(Relaxed) & mask != 0 {
                // Never let an Up through whose Down we ate, or the button would stick.
                swallowed = true;
                SWALLOWED_DOWN.fetch_and(!mask, Relaxed);
            }
            send(Ev::LlButton {
                ts,
                name,
                down,
                injected,
                extra: ms.dwExtraInfo,
                swallowed,
            });
        }
        if swallowed {
            LRESULT(1)
        } else {
            unsafe { CallNextHookEx(None, code, wparam, lparam) }
        }
    }

    // ---- logger / statistics ----
    /// Correlates hook moves with Raw Input mouse events of one source class.
    #[derive(Default)]
    struct Corr {
        last_ri: Option<i64>,
        /// Hook moves not yet matched: (ts, swallowed). Raw Input may still arrive after the hook.
        pending: Vec<(i64, bool)>,
        before: u32,
        after: u32,
        swallowed_matched: u32,
        /// `ri_ts - ll_ts` in µs (negative: Raw Input was processed first).
        deltas: Vec<i64>,
    }

    impl Corr {
        fn on_ll(&mut self, ts: i64, swallowed: bool) {
            self.pending
                .retain(|(t, _)| ts - t <= CORRELATION_WINDOW_US);
            if let Some(ri) = self.last_ri
                && ts - ri <= CORRELATION_WINDOW_US
            {
                self.before += 1;
                self.swallowed_matched += swallowed as u32;
                self.deltas.push(ri - ts);
                return;
            }
            self.pending.push((ts, swallowed));
        }

        fn on_ri(&mut self, ts: i64) {
            self.last_ri = Some(ts);
            let pending = std::mem::take(&mut self.pending);
            for (t, sw) in pending {
                if ts - t <= CORRELATION_WINDOW_US {
                    self.after += 1;
                    self.swallowed_matched += sw as u32;
                    self.deltas.push(ts - t);
                } else {
                    // Too old to ever match; dropped.
                }
            }
        }

        fn matched(&self) -> u32 {
            self.before + self.after
        }
    }

    fn median(v: &mut [i64]) -> Option<i64> {
        if v.is_empty() {
            return None;
        }
        v.sort_unstable();
        Some(v[v.len() / 2])
    }

    fn pct(n: u32, d: u32) -> String {
        if d == 0 {
            "n/a".into()
        } else {
            format!("{:.1}%", n as f64 * 100.0 / d as f64)
        }
    }

    fn logger(rx: Receiver<Ev>, suppress: bool, t0: i64) {
        let stdout = std::io::stdout();
        let mut out = std::io::BufWriter::new(stdout.lock());
        let rel = |ts: i64| ts - t0;

        let (mut ptp_events, mut mouse_tp, mut mouse_other) = (0u32, 0u32, 0u32);
        let (mut ll_moves, mut ll_moves_injected, mut ll_buttons) = (0u32, 0u32, 0u32);
        let (mut swallowed_moves, mut swallowed_buttons) = (0u32, 0u32);
        let (mut touchdowns, mut first_leak, mut first_held, mut touchdown_no_move) =
            (0u32, 0u32, 0u32, 0u32);
        let mut pre_touch_passed = 0u32;
        let mut awaiting_first = false;
        let mut prev_contacts = 0u32;
        let mut recent_passed: Vec<i64> = Vec::new();
        let (mut other_ri_touching, mut other_ri_touching_moving) = (0u32, 0u32);
        let (mut touching_now, mut ll_moves_touching) = (false, 0u32);
        let mut tp = Corr::default();
        let mut other = Corr::default();

        for ev in rx {
            match ev {
                Ev::Done => break,
                Ev::Info(s) => {
                    let _ = writeln!(out, "# {s}");
                }
                Ev::Ptp {
                    ts,
                    contacts,
                    announced,
                } => {
                    ptp_events += 1;
                    let _ = writeln!(
                        out,
                        "{:>12} RI-PTP contacts={contacts} (announced {announced})",
                        rel(ts)
                    );
                    if contacts > 0 && prev_contacts == 0 {
                        touchdowns += 1;
                        if awaiting_first {
                            touchdown_no_move += 1;
                        }
                        awaiting_first = true;
                        pre_touch_passed += recent_passed
                            .iter()
                            .filter(|&&t| ts - t <= PRE_TOUCH_WINDOW_US)
                            .count() as u32;
                    }
                    if contacts == 0 && prev_contacts > 0 && awaiting_first {
                        touchdown_no_move += 1;
                        awaiting_first = false;
                    }
                    prev_contacts = contacts;
                    touching_now = contacts > 0;
                }
                Ev::Mouse {
                    ts,
                    dev,
                    trackpad,
                    dx,
                    dy,
                    flags,
                } => {
                    let _ = writeln!(
                        out,
                        "{:>12} RI-MOUSE dev={dev:#x}{} dx={dx} dy={dy} buttons={flags:#06x}",
                        rel(ts),
                        if trackpad { " (trackpad)" } else { "" }
                    );
                    let moving = dx != 0 || dy != 0;
                    if trackpad {
                        mouse_tp += 1;
                        if moving {
                            tp.on_ri(ts);
                        }
                    } else {
                        mouse_other += 1;
                        if touching_now {
                            other_ri_touching += 1;
                            other_ri_touching_moving += moving as u32;
                        }
                        if moving {
                            other.on_ri(ts);
                        }
                    }
                }
                Ev::LlMove {
                    ts,
                    x,
                    y,
                    flags,
                    extra,
                    swallowed,
                    touching,
                } => {
                    let injected = flags & LLMHF_INJECTED_BIT != 0;
                    ll_moves += 1;
                    let _ = writeln!(
                        out,
                        "{:>12} LL move {x} {y} flags={flags:#x} injected={injected} extra={extra:#x}{}",
                        rel(ts),
                        if swallowed { " SWALLOWED" } else { "" }
                    );
                    if injected {
                        ll_moves_injected += 1;
                        continue;
                    }
                    if touching {
                        ll_moves_touching += 1;
                    }
                    if swallowed {
                        swallowed_moves += 1;
                    } else {
                        recent_passed.push(ts);
                        recent_passed.retain(|&t| ts - t <= PRE_TOUCH_WINDOW_US);
                    }
                    if awaiting_first && touching {
                        if swallowed {
                            first_held += 1;
                        } else {
                            first_leak += 1;
                        }
                        awaiting_first = false;
                    }
                    tp.on_ll(ts, swallowed);
                    other.on_ll(ts, swallowed);
                }
                Ev::LlButton {
                    ts,
                    name,
                    down,
                    injected,
                    extra,
                    swallowed,
                } => {
                    ll_buttons += 1;
                    swallowed_buttons += swallowed as u32;
                    let _ = writeln!(
                        out,
                        "{:>12} LL {name} {} injected={injected} extra={extra:#x}{}",
                        rel(ts),
                        if down { "down" } else { "up" },
                        if swallowed { " SWALLOWED" } else { "" }
                    );
                }
            }
        }

        let real_moves = ll_moves - ll_moves_injected;
        let mode = if suppress { "suppress" } else { "observe" };
        let _ = writeln!(out, "\n==== summary ({mode} mode) ====");
        let _ = writeln!(
            out,
            "Raw Input: PTP reports={ptp_events} mouse(trackpad)={mouse_tp} mouse(other)={mouse_other}"
        );
        let _ = writeln!(
            out,
            "WH_MOUSE_LL: moves={ll_moves} (injected {ll_moves_injected}) buttons={ll_buttons}"
        );
        match median(&mut tp.deltas) {
            Some(m) => {
                let _ = writeln!(
                    out,
                    "median RI->LL latency (trackpad mouse): {m} us (negative = Raw Input processed first, positive = hook first)"
                );
            }
            None => {
                let _ = writeln!(
                    out,
                    "median RI->LL latency: n/a (no hook move matched a trackpad Raw Input mouse event)"
                );
            }
        }
        let _ = writeln!(
            out,
            "correlation, non-injected LL moves vs trackpad RI-MOUSE within 2 ms: {} of {real_moves} ({}); RI first {}, hook first {}",
            tp.matched(),
            pct(tp.matched(), real_moves),
            tp.before,
            tp.after
        );
        let _ = writeln!(
            out,
            "correlation vs other (non-trackpad) RI-MOUSE within 2 ms: {} of {real_moves} ({})",
            other.matched(),
            pct(other.matched(), real_moves)
        );
        let _ = writeln!(
            out,
            "LL moves while a finger was down: {ll_moves_touching}; touch-downs: {touchdowns}"
        );
        if suppress {
            let _ = writeln!(
                out,
                "swallowed: moves={swallowed_moves} buttons={swallowed_buttons}"
            );
            let _ = writeln!(
                out,
                "first move after touch-down: leaked={first_leak} held={first_held} no-move={touchdown_no_move} (first-move-leak count = {first_leak})"
            );
            let _ = writeln!(
                out,
                "moves that passed the hook within {} ms before a touch-down was seen (race evidence): {pre_touch_passed}",
                PRE_TOUCH_WINDOW_US / 1000
            );
            let _ = writeln!(
                out,
                "other-mouse RI-MOUSE events while touching: {other_ri_touching} (moving {other_ri_touching_moving}); swallowed moves attributable to another mouse: {}",
                other.swallowed_matched
            );
            let _ = writeln!(
                out,
                "  -> if you moved a USB mouse while touching and the cursor froze, the hook cannot tell the sources apart."
            );
        }
        let _ = out.flush();
    }

    // ---- main ----
    /// `--seconds` is limited to 1..=3600.
    fn clamp_seconds(n: u64) -> u64 {
        n.clamp(1, 3600)
    }

    pub fn run() {
        let mut seconds = 20u64;
        let mut suppress = false;
        let mut args = std::env::args().skip(1);
        while let Some(a) = args.next() {
            match a.as_str() {
                "--suppress" => suppress = true,
                "--seconds" => match args.next().and_then(|s| s.parse().ok()) {
                    Some(n) => seconds = clamp_seconds(n),
                    None => return eprintln!("--seconds needs a number"),
                },
                "-h" | "--help" => {
                    return println!("usage: hook-probe [--suppress] [--seconds N]");
                }
                other => return eprintln!("unknown argument: {other}"),
            }
        }
        SUPPRESS.store(suppress, Relaxed);
        let (tx, rx) = channel();
        let _ = TX.set(tx);
        let t0 = now_us();
        DEADLINE_US.store(t0 + seconds as i64 * 1_000_000, Relaxed);
        let log = std::thread::spawn(move || logger(rx, suppress, t0));

        send(Ev::Info(format!(
            "hook-probe: mode={} seconds={seconds}",
            if suppress { "suppress" } else { "observe" }
        )));
        if let Err(e) = message_loop() {
            eprintln!("hook-probe failed: {e}");
        }
        send(Ev::Done);
        let _ = log.join();
    }

    fn message_loop() -> windows::core::Result<()> {
        unsafe {
            let hinstance: HINSTANCE = GetModuleHandleW(None)?.into();
            let class = w!("YubiHookProbeWindow");
            let wc = WNDCLASSW {
                lpfnWndProc: Some(wndproc),
                hInstance: hinstance,
                lpszClassName: class,
                ..Default::default()
            };
            if RegisterClassW(&wc) == 0 {
                return Err(windows::core::Error::from_thread());
            }
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                class,
                PCWSTR::null(),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                Some(hinstance),
                None,
            )?;
            let rid = [
                RAWINPUTDEVICE {
                    usUsagePage: PAGE_DIGITIZER,
                    usUsage: USAGE_TOUCH_PAD,
                    dwFlags: RIDEV_INPUTSINK | RIDEV_DEVNOTIFY,
                    hwndTarget: hwnd,
                },
                RAWINPUTDEVICE {
                    usUsagePage: 0x01,
                    usUsage: 0x02,
                    dwFlags: RIDEV_INPUTSINK | RIDEV_DEVNOTIFY,
                    hwndTarget: hwnd,
                },
            ];
            RegisterRawInputDevices(&rid, size_of::<RAWINPUTDEVICE>() as u32)?;
            enumerate_existing();
            let hook = SetWindowsHookExW(WH_MOUSE_LL, Some(ll_proc), Some(hinstance), 0)?;
            SetTimer(Some(hwnd), 1, 50, None);

            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            let _ = UnhookWindowsHookEx(hook);
            let _ = DestroyWindow(hwnd);
        }
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn seconds_are_clamped() {
            assert_eq!(clamp_seconds(0), 1);
            assert_eq!(clamp_seconds(20), 20);
            assert_eq!(clamp_seconds(u64::MAX), 3600);
        }
    }
}
