//! Display brightness: the internal panel through WMI, external monitors through DDC/CI.
//!
//! * Internal panel: `root\wmi` `WmiMonitorBrightness` (read) and
//!   `WmiMonitorBrightnessMethods.WmiSetBrightness` (write), levels 0..100.
//! * External monitors: `dxva2` high-level monitor API on the monitor under the cursor; the
//!   device min..max range is mapped to 0.0..1.0.
//!
//! Threading: like `volume`, [`Brightness`] holds COM interfaces and must stay on the thread that
//! opened it (COM is initialised once per thread and never uninitialised).
//!
//! DDC/CI is slow (roughly 50 ms or more per call and it can block much longer on a flaky
//! monitor). Never call it from the engine / UI thread: use [`BrightnessWorker`], which owns its
//! own thread and coalesces queued sets (latest value wins), so dragging a slider never builds a
//! backlog.
#![allow(dead_code)]

use std::fmt;
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use windows::Win32::Devices::Display::{
    DestroyPhysicalMonitors, GetMonitorBrightness, GetNumberOfPhysicalMonitorsFromHMONITOR,
    GetPhysicalMonitorsFromHMONITOR, PHYSICAL_MONITOR, SetMonitorBrightness,
};
use windows::Win32::Foundation::POINT;
use windows::Win32::Graphics::Gdi::{MONITOR_DEFAULTTONEAREST, MonitorFromPoint};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
    CoSetProxyBlanket, EOAC_NONE, RPC_C_AUTHN_LEVEL_CALL, RPC_C_IMP_LEVEL_IMPERSONATE,
};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::System::Wmi::{
    IWbemClassObject, IWbemContext, IWbemLocator, IWbemServices, WBEM_FLAG_FORWARD_ONLY,
    WBEM_FLAG_RETURN_IMMEDIATELY, WBEM_GENERIC_FLAG_TYPE, WbemLocator,
};
use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;
use windows::core::{BSTR, PCWSTR};

use super::volume::step_value;

/// Brightness failures are values, never panics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrightnessError {
    /// Neither an internal panel (WMI) nor a DDC/CI monitor under the cursor is available.
    Unsupported,
    /// An OS call failed.
    Os(String),
}

impl fmt::Display for BrightnessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported => f.write_str("brightness control is not supported on this display"),
            Self::Os(m) => write!(f, "brightness: {m}"),
        }
    }
}

impl std::error::Error for BrightnessError {}

impl From<windows::core::Error> for BrightnessError {
    fn from(e: windows::core::Error) -> Self {
        Self::Os(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, BrightnessError>;

// ---------------------------------------------------------------------------------------------
// Pure mapping

/// WMI level (0..=100) to 0.0..=1.0.
pub fn wmi_to_unit(level: u32) -> f32 {
    (level.min(100) as f32) / 100.0
}

/// 0.0..=1.0 to a WMI level (0..=100). Non-finite input maps to 0.
pub fn unit_to_wmi(v: f32) -> u32 {
    if !v.is_finite() {
        return 0;
    }
    (v.clamp(0.0, 1.0) * 100.0).round() as u32
}

/// DDC `min..=max` reading to 0.0..=1.0. A degenerate range (`max <= min`) yields 0.0.
pub fn ddc_to_unit(min: u32, cur: u32, max: u32) -> f32 {
    if max <= min {
        return 0.0;
    }
    (cur.clamp(min, max) - min) as f32 / (max - min) as f32
}

/// 0.0..=1.0 to a DDC value within `min..=max`. Non-finite input maps to `min`.
pub fn unit_to_ddc(min: u32, max: u32, v: f32) -> u32 {
    if max <= min || !v.is_finite() {
        return min;
    }
    min + (v.clamp(0.0, 1.0) * (max - min) as f32).round() as u32
}

// ---------------------------------------------------------------------------------------------
// COM helpers

thread_local! {
    static COM_INIT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// `CoInitializeEx` at most once per thread; never uninitialised (see the module doc).
fn init_com_once() {
    COM_INIT.with(|done| {
        if !done.get() {
            // S_FALSE / RPC_E_CHANGED_MODE just mean COM is already set up on this thread.
            // SAFETY: plain call.
            let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
            done.set(true);
        }
    });
}

/// Finite timeout for WMI enumeration so a stuck provider cannot hang the worker forever.
const WMI_TIMEOUT_MS: i32 = 2000;

const NO_FLAGS: WBEM_GENERIC_FLAG_TYPE = WBEM_GENERIC_FLAG_TYPE(0);

fn no_ctx() -> Option<&'static IWbemContext> {
    None
}

/// Connection to `root\wmi`.
struct Wmi {
    svc: IWbemServices,
}

impl Wmi {
    fn connect() -> Result<Self> {
        // SAFETY: standard WMI activation; interfaces are reference counted.
        unsafe {
            init_com_once();
            let loc: IWbemLocator = CoCreateInstance(&WbemLocator, None, CLSCTX_INPROC_SERVER)?;
            let svc = loc.ConnectServer(
                &BSTR::from("ROOT\\WMI"),
                &BSTR::new(),
                &BSTR::new(),
                &BSTR::new(),
                0,
                &BSTR::new(),
                no_ctx(),
            )?;
            CoSetProxyBlanket(
                &svc,
                10, // RPC_C_AUTHN_WINNT
                0,  // RPC_C_AUTHZ_NONE
                PCWSTR::null(),
                RPC_C_AUTHN_LEVEL_CALL,
                RPC_C_IMP_LEVEL_IMPERSONATE,
                None,
                EOAC_NONE,
            )?;
            Ok(Self { svc })
        }
    }

    /// First active instance of `class`, if any.
    fn first(&self, class: &str) -> Result<Option<IWbemClassObject>> {
        let query = BSTR::from(format!("SELECT * FROM {class} WHERE Active=TRUE"));
        // SAFETY: valid interfaces; the output slice outlives the call.
        unsafe {
            let en = self.svc.ExecQuery(
                &BSTR::from("WQL"),
                &query,
                WBEM_FLAG_FORWARD_ONLY | WBEM_FLAG_RETURN_IMMEDIATELY,
                no_ctx(),
            )?;
            let mut objs: [Option<IWbemClassObject>; 1] = [None];
            let mut returned = 0u32;
            let hr = en.Next(WMI_TIMEOUT_MS, &mut objs, &mut returned);
            if hr.is_err() {
                return Err(BrightnessError::Os(format!(
                    "WMI enumeration failed: {hr:?}"
                )));
            }
            Ok(if returned == 0 { None } else { objs[0].take() })
        }
    }

    fn get(&self) -> Result<f32> {
        let obj = self
            .first("WmiMonitorBrightness")?
            .ok_or(BrightnessError::Unsupported)?;
        let mut v = VARIANT::default();
        // SAFETY: valid object; `v` is an owned VARIANT.
        unsafe {
            obj.Get(
                windows::core::w!("CurrentBrightness"),
                0,
                &mut v,
                None,
                None,
            )?
        };
        let level = u32::try_from(&v)?;
        Ok(wmi_to_unit(level))
    }

    fn set(&self, value: f32) -> Result<()> {
        let inst = self
            .first("WmiMonitorBrightnessMethods")?
            .ok_or(BrightnessError::Unsupported)?;
        // SAFETY: valid interfaces and owned VARIANTs throughout.
        unsafe {
            let mut path = VARIANT::default();
            inst.Get(windows::core::w!("__PATH"), 0, &mut path, None, None)?;
            let path = BSTR::try_from(&path)?;

            let mut class = None;
            self.svc.GetObject(
                &BSTR::from("WmiMonitorBrightnessMethods"),
                NO_FLAGS,
                no_ctx(),
                Some(&mut class),
                None,
            )?;
            let class = class.ok_or(BrightnessError::Unsupported)?;
            let mut in_sig: Option<IWbemClassObject> = None;
            class.GetMethod(
                windows::core::w!("WmiSetBrightness"),
                0,
                &mut in_sig,
                std::ptr::null_mut(),
            )?;
            let params = in_sig
                .ok_or(BrightnessError::Unsupported)?
                .SpawnInstance(0)?;
            params.Put(windows::core::w!("Timeout"), 0, &VARIANT::from(0i32), 0)?;
            params.Put(
                windows::core::w!("Brightness"),
                0,
                &VARIANT::from(unit_to_wmi(value) as i32),
                0,
            )?;
            self.svc.ExecMethod(
                &path,
                &BSTR::from("WmiSetBrightness"),
                NO_FLAGS,
                no_ctx(),
                &params,
                None,
                None,
            )?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------------
// DDC/CI

/// Runs `f` with the first physical monitor of the monitor under the cursor, then releases it.
fn with_cursor_monitor<T>(f: impl FnOnce(&PHYSICAL_MONITOR) -> Result<T>) -> Result<T> {
    // SAFETY: plain Win32 calls; the physical monitor handles are destroyed before returning.
    unsafe {
        let mut pt = POINT::default();
        GetCursorPos(&mut pt)?;
        let hmon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
        let mut n = 0u32;
        GetNumberOfPhysicalMonitorsFromHMONITOR(hmon, &mut n)
            .map_err(|_| BrightnessError::Unsupported)?;
        if n == 0 {
            return Err(BrightnessError::Unsupported);
        }
        let mut mons = vec![PHYSICAL_MONITOR::default(); n as usize];
        GetPhysicalMonitorsFromHMONITOR(hmon, &mut mons)
            .map_err(|_| BrightnessError::Unsupported)?;
        let r = f(&mons[0]);
        let _ = DestroyPhysicalMonitors(&mons);
        r
    }
}

fn ddc_range(m: &PHYSICAL_MONITOR) -> Result<(u32, u32, u32)> {
    let (mut min, mut cur, mut max) = (0u32, 0u32, 0u32);
    // SAFETY: valid handle and out pointers.
    let ok = unsafe { GetMonitorBrightness(m.hPhysicalMonitor, &mut min, &mut cur, &mut max) };
    if ok == 0 || max <= min {
        return Err(BrightnessError::Unsupported);
    }
    Ok((min, cur, max))
}

fn ddc_get() -> Result<f32> {
    with_cursor_monitor(|m| {
        let (min, cur, max) = ddc_range(m)?;
        Ok(ddc_to_unit(min, cur, max))
    })
}

fn ddc_set(value: f32) -> Result<()> {
    with_cursor_monitor(|m| {
        let (min, _, max) = ddc_range(m)?;
        // SAFETY: valid handle.
        let ok = unsafe { SetMonitorBrightness(m.hPhysicalMonitor, unit_to_ddc(min, max, value)) };
        if ok == 0 {
            Err(BrightnessError::Os("SetMonitorBrightness failed".into()))
        } else {
            Ok(())
        }
    })
}

// ---------------------------------------------------------------------------------------------
// Public API

enum Backend {
    Wmi(Wmi),
    /// Resolved against the monitor under the cursor on every call.
    Ddc,
}

pub struct Brightness {
    backend: Backend,
}

impl Brightness {
    /// Prefer the internal panel (WMI); otherwise DDC/CI on the monitor under the cursor.
    /// `Err(Unsupported)` when neither works. May block (DDC): do not call on the engine thread.
    pub fn open() -> Result<Self> {
        if let Ok(w) = Wmi::connect()
            && w.get().is_ok()
        {
            return Ok(Self {
                backend: Backend::Wmi(w),
            });
        }
        ddc_get()?;
        Ok(Self {
            backend: Backend::Ddc,
        })
    }

    /// Brightness 0.0..=1.0.
    pub fn get(&self) -> Result<f32> {
        match &self.backend {
            Backend::Wmi(w) => w.get(),
            Backend::Ddc => ddc_get(),
        }
    }

    /// Set brightness (clamped to 0.0..=1.0).
    pub fn set(&self, v: f32) -> Result<()> {
        let v = v.clamp(0.0, 1.0);
        match &self.backend {
            Backend::Wmi(w) => w.set(v),
            Backend::Ddc => ddc_set(v),
        }
    }

    /// Add `delta` (e.g. 0.05); returns the new value.
    pub fn step(&self, delta: f32) -> Result<f32> {
        let v = step_value(self.get()?, delta);
        self.set(v)?;
        Ok(v)
    }
}

// ---------------------------------------------------------------------------------------------
// Worker

/// Work for the worker thread. Steps are relative and add up, so two quick gestures both count and
/// the base is always a fresh read taken by the worker (a change made with the monitor's own
/// buttons is never overwritten with a stale value).
#[derive(Debug, Default, PartialEq)]
struct Mailbox {
    /// Sum of the steps not taken yet, as a fraction of the full range.
    pending_step: Option<f32>,
    /// Counts every queued step.
    seq: u64,
    quit: bool,
}

impl Mailbox {
    /// Queue a step (added to one not yet taken). Returns its sequence number.
    fn push_step(&mut self, delta: f32) -> u64 {
        let sum = self.pending_step.unwrap_or(0.0) + delta;
        self.pending_step = Some(sum.clamp(-1.0, 1.0));
        self.seq += 1;
        self.seq
    }

    fn has_work(&self) -> bool {
        self.pending_step.is_some() || self.quit
    }

    /// Take the pending work: (step to apply with the sequence number of the newest step in it,
    /// quit).
    fn take(&mut self) -> (Option<(f32, u64)>, bool) {
        (self.pending_step.take().map(|d| (d, self.seq)), self.quit)
    }
}

/// What the worker last learned.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WorkerStatus {
    /// Last known brightness 0.0..=1.0 (after a successful step).
    pub value: Option<f32>,
    /// When `value` was learned.
    pub updated: Option<Instant>,
    /// Last error, cleared by the next success.
    pub error: Option<BrightnessError>,
    /// The newest step sequence number the worker has finished with (applied or failed). A step
    /// with a higher number is still waiting.
    pub applied_seq: u64,
}

type Shared = Arc<(Mutex<Mailbox>, Condvar)>;

/// Runs [`Brightness`] on its own thread so slow DDC/CI calls never block the caller.
pub struct BrightnessWorker {
    shared: Shared,
    status: Arc<Mutex<WorkerStatus>>,
    thread: Option<JoinHandle<()>>,
    /// Signalled by the worker thread when it returns.
    done: Option<mpsc::Receiver<()>>,
}

impl BrightnessWorker {
    pub fn spawn() -> Self {
        let shared: Shared = Arc::default();
        let status = Arc::new(Mutex::new(WorkerStatus::default()));
        let (s2, st2) = (shared.clone(), status.clone());
        let (done_tx, done_rx) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("yubi-brightness".into())
            .spawn(move || {
                worker_main(s2, st2);
                let _ = done_tx.send(());
            });
        let (thread, done) = match spawned {
            Ok(t) => (Some(t), Some(done_rx)),
            Err(e) => {
                status.lock().unwrap_or_else(|e| e.into_inner()).error = Some(BrightnessError::Os(
                    format!("明るさ用のスレッドを開始できません: {e}"),
                ));
                (None, None)
            }
        };
        Self {
            shared,
            status,
            thread,
            done,
        }
    }

    /// Queue a relative step (a fraction of the range). Never blocks. The worker reads the
    /// brightness fresh, adds the step and sets it. Returns the step's sequence number (compare
    /// with [`WorkerStatus::applied_seq`]).
    pub fn step(&self, delta: f32) -> u64 {
        let (m, cv) = &*self.shared;
        let seq = m.lock().unwrap_or_else(|e| e.into_inner()).push_step(delta);
        cv.notify_one();
        seq
    }

    pub fn status(&self) -> WorkerStatus {
        self.status
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

impl Drop for BrightnessWorker {
    fn drop(&mut self) {
        {
            let (m, cv) = &*self.shared;
            m.lock().unwrap_or_else(|e| e.into_inner()).quit = true;
            cv.notify_one();
        }
        // Wait briefly; a worker stuck in a slow WMI / DDC call is detached so exit never hangs.
        if let Some(t) = self.thread.take() {
            let finished = self
                .done
                .as_ref()
                .is_some_and(|rx| rx.recv_timeout(Duration::from_secs(1)).is_ok());
            if finished {
                let _ = t.join();
            }
        }
    }
}

fn worker_main(shared: Shared, status: Arc<Mutex<WorkerStatus>>) {
    let (m, cv) = &*shared;
    let mut dev: Option<Brightness> = None;
    loop {
        let (step, quit) = {
            let mut g = m.lock().unwrap_or_else(|e| e.into_inner());
            while !g.has_work() {
                g = cv.wait(g).unwrap_or_else(|e| e.into_inner());
            }
            g.take()
        };
        if quit {
            return;
        }
        let result = (|| {
            if dev.is_none() {
                dev = Some(Brightness::open()?);
            }
            let d = dev.as_ref().ok_or(BrightnessError::Unsupported)?;
            match step {
                Some((delta, _)) => {
                    let v = step_value(d.get()?, delta);
                    d.set(v).map(|()| Some(v))
                }
                None => Ok(None),
            }
        })();
        let mut st = status.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((_, seq)) = step {
            st.applied_seq = seq;
        }
        match result {
            Ok(Some(v)) => {
                st.value = Some(v);
                st.updated = Some(Instant::now());
                st.error = None;
            }
            Ok(None) => {}
            Err(e) => {
                // Reopen next time (the monitor under the cursor may have changed).
                dev = None;
                st.error = Some(e);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wmi_mapping() {
        assert_eq!(wmi_to_unit(0), 0.0);
        assert_eq!(wmi_to_unit(50), 0.5);
        assert_eq!(wmi_to_unit(250), 1.0);
        assert_eq!(unit_to_wmi(0.5), 50);
        assert_eq!(unit_to_wmi(2.0), 100);
        assert_eq!(unit_to_wmi(-1.0), 0);
        assert_eq!(unit_to_wmi(f32::NAN), 0);
    }

    #[test]
    fn ddc_mapping() {
        assert_eq!(ddc_to_unit(0, 50, 100), 0.5);
        assert_eq!(ddc_to_unit(10, 10, 110), 0.0);
        assert_eq!(ddc_to_unit(10, 110, 110), 1.0);
        assert_eq!(ddc_to_unit(10, 500, 110), 1.0);
        assert_eq!(ddc_to_unit(5, 5, 5), 0.0);
        assert_eq!(unit_to_ddc(10, 110, 0.0), 10);
        assert_eq!(unit_to_ddc(10, 110, 1.0), 110);
        assert_eq!(unit_to_ddc(10, 110, 0.5), 60);
        assert_eq!(unit_to_ddc(10, 110, 9.0), 110);
        assert_eq!(unit_to_ddc(10, 110, f32::NAN), 10);
    }

    #[test]
    fn mailbox_adds_steps_and_numbers_them() {
        let mut m = Mailbox::default();
        assert!(!m.has_work());
        assert_eq!(m.push_step(0.05), 1);
        assert_eq!(m.push_step(0.05), 2);
        assert_eq!(m.push_step(-0.02), 3);
        assert!(m.has_work());
        let (step, quit) = m.take();
        let (d, seq) = step.unwrap();
        assert!((d - 0.08).abs() < 1e-6, "{d}");
        assert_eq!(seq, 3);
        assert!(!quit);
        assert!(!m.has_work());
        // The sum is bounded, and numbering goes on.
        m.push_step(5.0);
        assert_eq!(m.take().0, Some((1.0, 4)));
        m.quit = true;
        assert_eq!(m.take(), (None, true));
    }

    /// Manual: `cargo test -- --ignored brightness_roundtrip --nocapture`.
    /// Reads the brightness, nudges it and restores the original value.
    #[test]
    #[ignore]
    fn brightness_roundtrip() {
        let b = match Brightness::open() {
            Ok(b) => b,
            Err(e) => {
                println!("skipped: {e}");
                return;
            }
        };
        let before = b.get().expect("get");
        println!("brightness = {before}");
        let nudged = if before > 0.5 {
            before - 0.1
        } else {
            before + 0.1
        };
        b.set(nudged).expect("set");
        b.set(before).expect("restore");
        assert!((b.get().unwrap() - before).abs() < 0.03);
    }
}
