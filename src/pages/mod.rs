pub mod diagnostics;
pub mod features;
pub mod general;
pub mod general_io;
pub mod home;

use crate::nav::Page;

/// Something a page asks the app shell to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cmd {
    Goto(Page),
    OpenConfigFolder,
    DisableWindowsGestures,
    RestoreWindowsGestures,
}
