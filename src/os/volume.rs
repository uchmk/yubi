//! Master volume of the default render device (Core Audio endpoint volume).
//!
//! COM: `Volume::open` calls `CoInitializeEx(COINIT_MULTITHREADED)` once per
//! thread (tracked in a thread-local flag; an already-initialised thread, including one in a
//! different apartment mode, is tolerated). COM is deliberately left initialised for the life of
//! the thread: there is no matching `CoUninitialize`, so `Volume` values can be dropped and
//! reopened freely without unbalancing the init count. The returned `Volume` holds COM interfaces
//! and must be used only on the thread that opened it. If the default output
//! device changes, drop it and `open` again.

#![allow(dead_code, reason = "Volume and Brightness gestures arrive in M3b")]

use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{IMMDeviceEnumerator, MMDeviceEnumerator, eConsole, eRender};
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
};
use windows::core::Result;

pub struct Volume {
    ep: IAudioEndpointVolume,
}

/// New scalar after adding `delta`, clamped to 0.0..=1.0.
pub fn step_value(current: f32, delta: f32) -> f32 {
    (current + delta).clamp(0.0, 1.0)
}

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

impl Volume {
    pub fn open() -> Result<Self> {
        // SAFETY: standard COM activation; interfaces are reference counted.
        unsafe {
            init_com_once();
            let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
            let dev = en.GetDefaultAudioEndpoint(eRender, eConsole)?;
            let ep: IAudioEndpointVolume = dev.Activate(CLSCTX_ALL, None)?;
            Ok(Self { ep })
        }
    }

    /// Master volume scalar 0.0..=1.0.
    pub fn get(&self) -> Result<f32> {
        // SAFETY: valid interface.
        unsafe { self.ep.GetMasterVolumeLevelScalar() }
    }

    /// Set master volume (clamped to 0.0..=1.0). Events carry no yubi context.
    pub fn set(&self, v: f32) -> Result<()> {
        // SAFETY: valid interface; null event context is allowed.
        unsafe {
            self.ep
                .SetMasterVolumeLevelScalar(v.clamp(0.0, 1.0), std::ptr::null())
        }
    }

    /// Add `delta` (e.g. 0.02) to the volume; returns the new value.
    pub fn step(&self, delta: f32) -> Result<f32> {
        let v = step_value(self.get()?, delta);
        self.set(v)?;
        Ok(v)
    }

    pub fn is_muted(&self) -> Result<bool> {
        // SAFETY: valid interface.
        unsafe { Ok(self.ep.GetMute()?.as_bool()) }
    }

    pub fn set_muted(&self, muted: bool) -> Result<()> {
        // SAFETY: valid interface; null event context is allowed.
        unsafe { self.ep.SetMute(muted, std::ptr::null()) }
    }

    pub fn toggle_mute(&self) -> Result<bool> {
        let m = !self.is_muted()?;
        self.set_muted(m)?;
        Ok(m)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_clamps() {
        assert_eq!(step_value(0.5, 0.1), 0.6);
        assert_eq!(step_value(0.95, 0.1), 1.0);
        assert_eq!(step_value(0.05, -0.1), 0.0);
    }

    /// Manual: `cargo test -- --ignored volume_roundtrip --nocapture`.
    /// Reads the volume, nudges it by one step and restores it.
    #[test]
    #[ignore]
    fn volume_roundtrip() {
        let v = Volume::open().expect("open");
        let before = v.get().expect("get");
        println!("volume = {before}");
        v.step(0.01).expect("step");
        v.set(before).expect("restore");
        assert!((v.get().unwrap() - before).abs() < 0.01);
    }
}
