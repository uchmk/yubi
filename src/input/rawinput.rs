//! Windows Raw Input reader for HID touch pads (Usage Page 0x0D, Usage 0x05).
//!
//! A dedicated thread owns a message-only window. Every `WM_INPUT` report is
//! decoded with the Windows HID parser (`HidP_*` and the device's preparsed
//! data), never with hard-coded byte offsets.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use windows::Win32::Devices::HumanInterfaceDevice::*;
use windows::Win32::Foundation::*;
use windows::Win32::Storage::FileSystem::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

use super::{Contact, DeviceInfo, FrameAccumulator, InputEvent, TouchFrame, normalize};

const PAGE_GENERIC_DESKTOP: u16 = 0x01;
const PAGE_BUTTON: u16 = 0x09;
const PAGE_DIGITIZER: u16 = 0x0D;
const USAGE_TOUCH_PAD: u16 = 0x05;
const USAGE_X: u16 = 0x30;
const USAGE_Y: u16 = 0x31;
const USAGE_TIP_PRESSURE: u16 = 0x30;
const USAGE_TIP_SWITCH: u16 = 0x42;
const USAGE_CONFIDENCE: u16 = 0x47;
const USAGE_CONTACT_ID: u16 = 0x51;
const USAGE_CONTACT_COUNT: u16 = 0x54;
const USAGE_SCAN_TIME: u16 = 0x56;

const GIDC_ARRIVAL: usize = 1;
const GIDC_REMOVAL: usize = 2;

#[derive(Clone, Copy)]
struct Axis {
    min: i32,
    max: i32,
    bits: u16,
}

impl Axis {
    fn from_caps(c: &HIDP_VALUE_CAPS) -> Self {
        Self {
            min: c.LogicalMin,
            max: c.LogicalMax,
            bits: c.BitSize,
        }
    }

    /// Sign-extend a raw field value when the logical range is signed.
    fn decode(&self, raw: u32) -> i32 {
        if self.min < 0 && (1..32).contains(&self.bits) {
            let shift = 32 - u32::from(self.bits);
            ((raw << shift) as i32) >> shift
        } else {
            raw as i32
        }
    }
}

/// Where each field of a touch pad report lives (link collections).
struct Layout {
    fingers: Vec<u16>,
    x: Axis,
    y: Axis,
    pressure: Option<Axis>,
    count_link: Option<u16>,
    scan_link: Option<u16>,
    button_links: Vec<u16>,
}

struct Device {
    /// Keeps the preparsed data alive (8-byte aligned).
    preparsed: Vec<u64>,
    layout: Layout,
    info: Arc<DeviceInfo>,
    acc: FrameAccumulator,
}

impl Device {
    fn pd(&self) -> PHIDP_PREPARSED_DATA {
        PHIDP_PREPARSED_DATA(self.preparsed.as_ptr() as isize)
    }
}

struct State {
    tx: Sender<InputEvent>,
    wake: Box<dyn Fn() + Send>,
    devices: HashMap<usize, Option<Device>>,
    log: bool,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

pub fn spawn(wake: impl Fn() + Send + 'static) -> Receiver<InputEvent> {
    let (tx, rx) = channel();
    let wake: Box<dyn Fn() + Send> = Box::new(wake);
    let spawned = std::thread::Builder::new()
        .name("yubi-rawinput".into())
        .spawn(move || {
            if let Err(e) = run(tx, wake) {
                eprintln!("yubi: raw input thread failed: {e}");
            }
        });
    if let Err(e) = spawned {
        eprintln!("yubi: cannot start raw input thread: {e}");
    }
    rx
}

fn run(tx: Sender<InputEvent>, wake: Box<dyn Fn() + Send>) -> windows::core::Result<()> {
    let log = std::env::var_os("YUBI_LOG_INPUT").is_some();
    STATE.with(|s| {
        *s.borrow_mut() = Some(State {
            tx,
            wake,
            devices: HashMap::new(),
            log,
        });
    });
    unsafe {
        let hinstance = GetModuleHandleW(None)?.into();
        let class = w!("YubiRawInputWindow");
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
            w!("yubi-input"),
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
        let rid = [RAWINPUTDEVICE {
            usUsagePage: PAGE_DIGITIZER,
            usUsage: USAGE_TOUCH_PAD,
            dwFlags: RIDEV_INPUTSINK | RIDEV_DEVNOTIFY,
            hwndTarget: hwnd,
        }];
        RegisterRawInputDevices(&rid, size_of::<RAWINPUTDEVICE>() as u32)?;
        enumerate_existing();

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    Ok(())
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_INPUT => {
            handle_input(HRAWINPUT(lparam.0 as *mut _));
        }
        WM_INPUT_DEVICE_CHANGE => {
            let handle = HANDLE(lparam.0 as *mut _);
            match wparam.0 {
                GIDC_ARRIVAL => with_state(|st| {
                    st.devices.remove(&(handle.0 as usize));
                    ensure_device(st, handle);
                }),
                GIDC_REMOVAL => with_state(|st| {
                    if let Some(Some(dev)) = st.devices.remove(&(handle.0 as usize)) {
                        st.emit(InputEvent::DeviceRemoved(dev.info.name.clone()));
                    }
                }),
                _ => {}
            }
        }
        _ => {}
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

fn with_state(f: impl FnOnce(&mut State)) {
    STATE.with(|s| {
        if let Some(st) = s.borrow_mut().as_mut() {
            f(st);
        }
    });
}

impl State {
    fn emit(&self, ev: InputEvent) {
        if self.tx.send(ev).is_ok() {
            (self.wake)();
        }
    }
}

/// Pick up touch pads that were already connected before we started.
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
        with_state(|st| {
            for entry in list.iter().take(got as usize) {
                if entry.dwType == RIM_TYPEHID {
                    ensure_device(st, entry.hDevice);
                }
            }
        });
    }
}

/// Build (once) the per-device state; non-touch-pad HID devices are cached as `None`.
fn ensure_device(st: &mut State, handle: HANDLE) -> Option<&mut Device> {
    let key = handle.0 as usize;
    if !st.devices.contains_key(&key) {
        let dev = unsafe { build_device(handle, st.log) };
        if let Some(d) = &dev {
            if st.log {
                eprintln!("yubi: device {:?} layout: {:?}", d.info.name, d.info);
            }
            st.emit(InputEvent::DeviceArrived(d.info.clone()));
        }
        st.devices.insert(key, dev);
    }
    st.devices.get_mut(&key)?.as_mut()
}

fn read_device_name(handle: HANDLE) -> Option<Vec<u16>> {
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
        Some(buf)
    }
}

/// Product string from the HID device (opened with no access rights), else the path.
fn product_name(path: &[u16]) -> String {
    let fallback = String::from_utf16_lossy(path);
    let mut wide = path.to_vec();
    wide.push(0);
    unsafe {
        let Ok(h) = CreateFileW(
            PCWSTR(wide.as_ptr()),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            None,
        ) else {
            return fallback;
        };
        let mut buf = [0u16; 127];
        let ok = HidD_GetProductString(h, buf.as_mut_ptr().cast(), size_of_val(&buf) as u32);
        let _ = CloseHandle(h);
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        if ok && end > 0 {
            String::from_utf16_lossy(&buf[..end])
        } else {
            fallback
        }
    }
}

unsafe fn build_device(handle: HANDLE, log: bool) -> Option<Device> {
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
        let layout = read_layout(pd, log);
        if layout.is_none() && log {
            eprintln!("yubi: hid device {handle:?} has no finger collections");
        }
        let layout = layout?;
        let name = read_device_name(handle)
            .map(|p| product_name(&p))
            .unwrap_or_else(|| "HID touch pad".to_string());
        let info = Arc::new(DeviceInfo {
            name,
            x_range: (layout.x.min, layout.x.max),
            y_range: (layout.y.min, layout.y.max),
            pressure_range: layout.pressure.map(|a| (a.min, a.max)),
            finger_slots: layout.fingers.len(),
        });
        Some(Device {
            preparsed,
            layout,
            info,
            acc: FrameAccumulator::default(),
        })
    }
}

/// Expand a value cap into the individual usages it describes.
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

unsafe fn read_layout(pd: PHIDP_PREPARSED_DATA, log: bool) -> Option<Layout> {
    unsafe {
        let mut caps = HIDP_CAPS::default();
        if HidP_GetCaps(pd, &mut caps) != HIDP_STATUS_SUCCESS {
            return None;
        }
        if log {
            eprintln!(
                "yubi: hid caps page={:#x} usage={:#x} valuecaps={} buttoncaps={}",
                caps.UsagePage, caps.Usage, caps.NumberInputValueCaps, caps.NumberInputButtonCaps
            );
        }
        let mut n = caps.NumberInputValueCaps;
        if n == 0 {
            return None;
        }
        let mut vcaps = vec![HIDP_VALUE_CAPS::default(); n as usize];
        if HidP_GetValueCaps(HidP_Input, vcaps.as_mut_ptr(), &mut n, pd) != HIDP_STATUS_SUCCESS {
            return None;
        }
        vcaps.truncate(n as usize);
        if log {
            for c in &vcaps {
                eprintln!(
                    "yubi:   value page={:#x} usages={:?} link={} (page {:#x} usage {:#x}) report_id={} bits={} count={} range={}..{}",
                    c.UsagePage,
                    cap_usages(c),
                    c.LinkCollection,
                    c.LinkUsagePage,
                    c.LinkUsage,
                    c.ReportID,
                    c.BitSize,
                    c.ReportCount,
                    c.LogicalMin,
                    c.LogicalMax
                );
            }
        }

        let find = |page: u16, usage: u16, link: Option<u16>| {
            vcaps.iter().find(|c| {
                c.UsagePage == page
                    && link.is_none_or(|l| c.LinkCollection == l)
                    && cap_usages(c).contains(&usage)
            })
        };

        // Finger collections: link collections that carry a Contact ID.
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
        let first = *fingers.first()?;

        let x = Axis::from_caps(find(PAGE_GENERIC_DESKTOP, USAGE_X, Some(first))?);
        let y = Axis::from_caps(find(PAGE_GENERIC_DESKTOP, USAGE_Y, Some(first))?);
        let pressure = find(PAGE_DIGITIZER, USAGE_TIP_PRESSURE, Some(first)).map(Axis::from_caps);
        let count_link = find(PAGE_DIGITIZER, USAGE_CONTACT_COUNT, None).map(|c| c.LinkCollection);
        let scan_link = find(PAGE_DIGITIZER, USAGE_SCAN_TIME, None).map(|c| c.LinkCollection);

        let mut button_links = Vec::new();
        let mut nb = caps.NumberInputButtonCaps;
        if nb > 0 {
            let mut bcaps = vec![HIDP_BUTTON_CAPS::default(); nb as usize];
            if HidP_GetButtonCaps(HidP_Input, bcaps.as_mut_ptr(), &mut nb, pd)
                == HIDP_STATUS_SUCCESS
            {
                bcaps.truncate(nb as usize);
                for b in bcaps.iter().filter(|b| b.UsagePage == PAGE_BUTTON) {
                    if !button_links.contains(&b.LinkCollection) {
                        button_links.push(b.LinkCollection);
                    }
                }
            }
        }
        Some(Layout {
            fingers,
            x,
            y,
            pressure,
            count_link,
            scan_link,
            button_links,
        })
    }
}

fn get_value(
    pd: PHIDP_PREPARSED_DATA,
    page: u16,
    link: u16,
    usage: u16,
    report: &[u8],
) -> Option<u32> {
    let mut v = 0u32;
    let st = unsafe { HidP_GetUsageValue(HidP_Input, page, Some(link), usage, &mut v, pd, report) };
    (st == HIDP_STATUS_SUCCESS).then_some(v)
}

/// Usages currently set (pressed) in one link collection / page.
fn get_usages(
    pd: PHIDP_PREPARSED_DATA,
    page: u16,
    link: u16,
    report: &mut [u8],
) -> Option<Vec<u16>> {
    let mut buf = [0u16; 32];
    let mut len = buf.len() as u32;
    let st = unsafe {
        HidP_GetUsages(
            HidP_Input,
            page,
            Some(link),
            buf.as_mut_ptr(),
            &mut len,
            pd,
            report,
        )
    };
    (st == HIDP_STATUS_SUCCESS).then(|| buf[..len as usize].to_vec())
}

/// A one-bit flag may be exposed as a button or as a value depending on the descriptor.
fn get_flag(pd: PHIDP_PREPARSED_DATA, link: u16, usage: u16, report: &mut [u8]) -> Option<bool> {
    if let Some(v) = get_value(pd, PAGE_DIGITIZER, link, usage, report) {
        return Some(v != 0);
    }
    get_usages(pd, PAGE_DIGITIZER, link, report).map(|set| set.contains(&usage))
}

fn decode_report(dev: &mut Device, report: &mut [u8]) -> Option<TouchFrame> {
    let pd = dev.pd();
    let lay = &dev.layout;

    // A report without a Contact ID for the first finger is not a touch report.
    get_value(pd, PAGE_DIGITIZER, lay.fingers[0], USAGE_CONTACT_ID, report)?;

    let announced = lay
        .count_link
        .and_then(|l| get_value(pd, PAGE_DIGITIZER, l, USAGE_CONTACT_COUNT, report))
        .unwrap_or(0) as usize;
    let scan_time = lay
        .scan_link
        .and_then(|l| get_value(pd, PAGE_DIGITIZER, l, USAGE_SCAN_TIME, report))
        .unwrap_or(0);
    let mut button = false;
    for &l in &lay.button_links {
        if let Some(set) = get_usages(pd, PAGE_BUTTON, l, report) {
            button |= set.contains(&1);
        }
    }

    let mut slots = Vec::with_capacity(lay.fingers.len());
    for &link in &lay.fingers {
        let Some(id) = get_value(pd, PAGE_DIGITIZER, link, USAGE_CONTACT_ID, report) else {
            continue;
        };
        let rx = get_value(pd, PAGE_GENERIC_DESKTOP, link, USAGE_X, report).unwrap_or(0);
        let ry = get_value(pd, PAGE_GENERIC_DESKTOP, link, USAGE_Y, report).unwrap_or(0);
        let pressure = lay.pressure.and_then(|a| {
            get_value(pd, PAGE_DIGITIZER, link, USAGE_TIP_PRESSURE, report)
                .map(|v| normalize(a.decode(v), a.min, a.max))
        });
        slots.push(Contact {
            id,
            x: normalize(lay.x.decode(rx), lay.x.min, lay.x.max),
            y: normalize(lay.y.decode(ry), lay.y.min, lay.y.max),
            tip: get_flag(pd, link, USAGE_TIP_SWITCH, report).unwrap_or(false),
            confidence: get_flag(pd, link, USAGE_CONFIDENCE, report).unwrap_or(true),
            pressure,
        });
    }
    // Pads that do not report Contact Count: treat every active slot as the frame.
    let (slots, announced) = if lay.count_link.is_none() {
        let active: Vec<Contact> = slots.into_iter().filter(|c| c.tip).collect();
        let n = active.len();
        (active, n)
    } else {
        (slots, announced)
    };
    let contacts = dev.acc.push(slots, announced)?;
    Some(TouchFrame {
        contacts,
        button,
        scan_time,
        device: dev.info.clone(),
    })
}

fn handle_input(hraw: HRAWINPUT) {
    unsafe {
        let mut size = 0u32;
        let header = size_of::<RAWINPUTHEADER>() as u32;
        if GetRawInputData(hraw, RID_INPUT, None, &mut size, header) == u32::MAX || size == 0 {
            return;
        }
        let mut buf = vec![0u64; (size as usize).div_ceil(8)];
        let got = GetRawInputData(
            hraw,
            RID_INPUT,
            Some(buf.as_mut_ptr().cast()),
            &mut size,
            header,
        );
        if got == u32::MAX {
            return;
        }
        let bytes = std::slice::from_raw_parts(buf.as_ptr().cast::<u8>(), size as usize);
        let hdr = &*(bytes.as_ptr().cast::<RAWINPUTHEADER>());
        if hdr.dwType != RIM_TYPEHID.0 {
            return;
        }
        let hid_off = header as usize;
        if bytes.len() < hid_off + 8 {
            return;
        }
        let size_hid = u32::from_ne_bytes(bytes[hid_off..hid_off + 4].try_into().unwrap()) as usize;
        let count =
            u32::from_ne_bytes(bytes[hid_off + 4..hid_off + 8].try_into().unwrap()) as usize;
        let data = &bytes[hid_off + 8..];
        if size_hid == 0 || data.len() < size_hid * count {
            return;
        }
        let handle = hdr.hDevice;
        with_state(|st| {
            let log = st.log;
            let mut frames = Vec::new();
            let Some(dev) = ensure_device(st, handle) else {
                if log {
                    eprintln!(
                        "yubi: WM_INPUT from unsupported device: {count} report(s) of {size_hid} bytes, id {:?}",
                        data.first()
                    );
                }
                return;
            };
            for i in 0..count {
                let mut report = data[i * size_hid..(i + 1) * size_hid].to_vec();
                match decode_report(dev, &mut report) {
                    Some(f) => frames.push(f),
                    None if log => eprintln!("yubi: report skipped ({} bytes)", report.len()),
                    None => {}
                }
            }
            for f in frames {
                if log {
                    eprintln!(
                        "yubi: frame n={} btn={} scan={} {:?}",
                        f.contacts.len(),
                        f.button,
                        f.scan_time,
                        f.contacts
                    );
                }
                st.emit(InputEvent::Frame(f));
            }
        });
    }
}
