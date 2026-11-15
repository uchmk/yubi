//! Executable name lookup for a window, cached per process id.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;
use windows::core::PWSTR;

use super::window::Hwnd;

/// How long a pid -> exe name result is trusted (pids can be reused).
const TTL: Duration = Duration::from_secs(30);
const MAX_ENTRIES: usize = 64;

/// Small TTL cache keyed by pid. Time is passed in so it can be unit-tested.
#[derive(Default)]
pub struct PidCache {
    map: HashMap<u32, (Instant, Option<String>)>,
}

impl PidCache {
    /// `None` = not cached / expired; `Some(None)` = cached "lookup failed".
    pub fn get(&self, pid: u32, now: Instant) -> Option<Option<String>> {
        let (at, v) = self.map.get(&pid)?;
        (now.duration_since(*at) < TTL).then(|| v.clone())
    }

    pub fn put(&mut self, pid: u32, now: Instant, value: Option<String>) {
        if self.map.len() >= MAX_ENTRIES && !self.map.contains_key(&pid) {
            self.map.retain(|_, (at, _)| now.duration_since(*at) < TTL);
            if self.map.len() >= MAX_ENTRIES {
                // Still full: evict the oldest entry.
                if let Some(oldest) = self
                    .map
                    .iter()
                    .min_by_key(|(_, (at, _))| *at)
                    .map(|(k, _)| *k)
                {
                    self.map.remove(&oldest);
                }
            }
        }
        self.map.insert(pid, (now, value));
    }

    #[allow(dead_code, reason = "only the cache tests look at its size")]
    pub fn len(&self) -> usize {
        self.map.len()
    }

    #[allow(dead_code, reason = "pairs with len")]
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

static CACHE: Mutex<Option<PidCache>> = Mutex::new(None);

/// Lowercase file name of a Windows path ("C:\\X\\Code.EXE" -> "code.exe").
pub fn file_name_lower(path: &str) -> Option<String> {
    let name = path.rsplit(['\\', '/']).next()?;
    (!name.is_empty()).then(|| name.to_lowercase())
}

/// Process id owning `hwnd`.
pub fn pid_of(hwnd: Hwnd) -> Option<u32> {
    let mut pid = 0u32;
    // SAFETY: valid out pointer.
    unsafe { GetWindowThreadProcessId(hwnd.to_win(), Some(&mut pid)) };
    (pid != 0).then_some(pid)
}

fn query_exe_path(pid: u32) -> Option<String> {
    // SAFETY: handle is closed below; buffer length is passed in/out.
    unsafe {
        let h: HANDLE = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = vec![0u16; 1024];
        let mut len = buf.len() as u32;
        let r =
            QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len);
        let _ = CloseHandle(h);
        r.ok()?;
        Some(String::from_utf16_lossy(&buf[..len as usize]))
    }
}

/// Lowercase executable file name of the process by pid (cached ~30 s).
pub fn exe_name_of_pid(pid: u32) -> Option<String> {
    let now = Instant::now();
    let mut guard = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let cache = guard.get_or_insert_with(PidCache::default);
    if let Some(v) = cache.get(pid, now) {
        return v;
    }
    let v = query_exe_path(pid).and_then(|p| file_name_lower(&p));
    cache.put(pid, now, v.clone());
    v
}

/// Lowercase executable name ("chrome.exe") of the process owning `hwnd`.
pub fn exe_name_of(hwnd: Hwnd) -> Option<String> {
    exe_name_of_pid(pid_of(hwnd)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_name_lowercased() {
        assert_eq!(
            file_name_lower(r"C:\Program Files\Foo\Code.EXE").as_deref(),
            Some("code.exe")
        );
        assert_eq!(file_name_lower("a.exe").as_deref(), Some("a.exe"));
        assert_eq!(file_name_lower(r"C:\dir\"), None);
    }

    #[test]
    fn cache_ttl_and_negative() {
        let mut c = PidCache::default();
        let t0 = Instant::now();
        assert_eq!(c.get(1, t0), None);
        c.put(1, t0, Some("a.exe".into()));
        c.put(2, t0, None);
        assert_eq!(c.get(1, t0), Some(Some("a.exe".into())));
        assert_eq!(c.get(2, t0), Some(None));
        assert_eq!(c.get(1, t0 + TTL + Duration::from_millis(1)), None);
    }

    #[test]
    fn cache_is_bounded() {
        let mut c = PidCache::default();
        let t0 = Instant::now();
        for i in 0..200u32 {
            c.put(i, t0 + Duration::from_millis(i as u64), Some("x".into()));
        }
        assert!(c.len() <= MAX_ENTRIES);
        // The newest entry survives, the oldest was evicted.
        assert!(c.get(199, t0 + Duration::from_millis(199)).is_some());
        assert!(c.get(0, t0 + Duration::from_millis(199)).is_none());
    }

    #[test]
    fn own_process_resolves() {
        let exe = exe_name_of_pid(std::process::id()).expect("own exe");
        assert!(exe.ends_with(".exe"), "{exe}");
    }
}
