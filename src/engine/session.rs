//! Session lock / suspend watcher.
//!
//! A thread owns a hidden top-level window (never shown; a message-only window would not get the
//! WM_POWERBROADCAST broadcast) and bumps a counter whenever the session locks, disconnects,
//! logs off or the PC suspends. The engine compares the counter every wake-up and releases
//! everything it holds. Untested without a real lock / suspend: see the hardware list.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Bumped on every "let go of everything" event.
pub type Interrupts = Arc<AtomicU64>;

pub struct SessionWatch {
    #[cfg(windows)]
    inner: win::Watch,
}

impl SessionWatch {
    /// `report` is called (from the watcher thread) with a Japanese message when the watcher
    /// cannot start or stops, so the UI can show that locking no longer releases held keys.
    pub fn start(interrupts: Interrupts, report: impl Fn(String) + Send + 'static) -> Self {
        #[cfg(windows)]
        {
            Self {
                inner: win::Watch::start(interrupts, report),
            }
        }
        #[cfg(not(windows))]
        {
            let _ = (interrupts, report);
            Self {}
        }
    }

    pub fn stop(&mut self) {
        #[cfg(windows)]
        self.inner.stop();
    }
}

#[cfg(windows)]
mod win {
    use std::cell::RefCell;
    use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::thread::JoinHandle;

    use windows::Win32::Foundation::*;
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::RemoteDesktop::{
        NOTIFY_FOR_THIS_SESSION, WTSRegisterSessionNotification, WTSUnRegisterSessionNotification,
    };
    use windows::Win32::UI::WindowsAndMessaging::*;
    use windows::core::w;

    use super::Interrupts;

    const WM_WTSSESSION_CHANGE: u32 = 0x02B1;
    const WTS_CONSOLE_DISCONNECT: usize = 2;
    const WTS_REMOTE_DISCONNECT: usize = 4;
    const WTS_SESSION_LOGOFF: usize = 6;
    const WTS_SESSION_LOCK: usize = 7;
    const PBT_APMSUSPEND: usize = 4;

    thread_local! {
        static INTERRUPTS: RefCell<Option<Interrupts>> = const { RefCell::new(None) };
    }

    const START_FAILED: &str = "開始できません（ロック時の解放が効きません）";

    struct Shared {
        hwnd: AtomicIsize,
        stop: AtomicBool,
    }

    pub struct Watch {
        shared: Arc<Shared>,
        join: Mutex<Option<JoinHandle<()>>>,
    }

    impl Watch {
        pub fn start(interrupts: Interrupts, report: impl Fn(String) + Send + 'static) -> Self {
            let shared = Arc::new(Shared {
                hwnd: AtomicIsize::new(0),
                stop: AtomicBool::new(false),
            });
            let thread_shared = shared.clone();
            let spawned = std::thread::Builder::new()
                .name("yubi-session".into())
                .spawn(move || {
                    INTERRUPTS.with(|i| *i.borrow_mut() = Some(interrupts));
                    match unsafe { run(&thread_shared) } {
                        Ok(true) => {}
                        Ok(false) => report(START_FAILED.to_string()),
                        Err(e) => {
                            eprintln!("yubi: session watcher failed: {e}");
                            report(START_FAILED.to_string());
                        }
                    }
                });
            let join = match spawned {
                Ok(j) => Some(j),
                Err(e) => {
                    eprintln!("yubi: cannot start session watcher: {e}");
                    None
                }
            };
            Self {
                shared,
                join: Mutex::new(join),
            }
        }

        pub fn stop(&mut self) {
            // Flag first: a thread that has not created its window yet sees it and exits.
            self.shared.stop.store(true, Ordering::SeqCst);
            let h = self.shared.hwnd.load(Ordering::SeqCst);
            if h != 0 {
                unsafe {
                    let _ = PostMessageW(Some(HWND(h as *mut _)), WM_CLOSE, WPARAM(0), LPARAM(0));
                }
            }
            if let Ok(mut j) = self.join.lock()
                && let Some(j) = j.take()
            {
                let _ = j.join();
            }
        }
    }

    /// Returns whether the session notification could be registered.
    unsafe fn run(shared: &Shared) -> windows::core::Result<bool> {
        unsafe {
            let hinstance = GetModuleHandleW(None)?.into();
            let class = w!("YubiSessionWatchWindow");
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
                w!("yubi-session"),
                WS_OVERLAPPED,
                0,
                0,
                0,
                0,
                None,
                None,
                Some(hinstance),
                None,
            )?;
            shared.hwnd.store(hwnd.0 as isize, Ordering::SeqCst);
            if shared.stop.load(Ordering::SeqCst) {
                let _ = DestroyWindow(hwnd);
                return Ok(true);
            }
            let registered = WTSRegisterSessionNotification(hwnd, NOTIFY_FOR_THIS_SESSION).is_ok();
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            if registered {
                let _ = WTSUnRegisterSessionNotification(hwnd);
            }
            Ok(registered)
        }
    }

    fn interrupt() {
        INTERRUPTS.with(|i| {
            if let Some(n) = i.borrow().as_ref() {
                n.fetch_add(1, Ordering::SeqCst);
            }
        });
    }

    unsafe extern "system" fn wndproc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_WTSSESSION_CHANGE
                if matches!(
                    wparam.0,
                    WTS_CONSOLE_DISCONNECT
                        | WTS_REMOTE_DISCONNECT
                        | WTS_SESSION_LOGOFF
                        | WTS_SESSION_LOCK
                ) =>
            {
                interrupt();
            }
            WM_POWERBROADCAST if wparam.0 == PBT_APMSUSPEND => interrupt(),
            WM_DESTROY => unsafe { PostQuitMessage(0) },
            _ => {}
        }
        unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
    }
}

/// Current value of the counter (for tests and the engine loop).
pub fn count(i: &Interrupts) -> u64 {
    i.load(Ordering::SeqCst)
}
