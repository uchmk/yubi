//! One yubi per user session: a named mutex; a second launch asks the first to show its window.

use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
use windows::Win32::System::Threading::CreateMutexW;
use windows::core::w;

/// Claim the instance. `true` when this process is the first one. The handle is kept until exit.
pub fn claim() -> bool {
    unsafe {
        // The handle is never closed: the mutex must live as long as the process.
        let Ok(_handle) = CreateMutexW(None, false, w!("Local\\yubi-single-instance")) else {
            // Cannot tell; run rather than refuse to start.
            return true;
        };
        let already = GetLastError() == ERROR_ALREADY_EXISTS;
        !already
    }
}
