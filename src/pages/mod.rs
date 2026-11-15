pub mod apps;
pub mod diagnostics;
pub mod features;
pub mod general;
pub mod general_io;
pub mod home;
pub mod overview;
pub mod practice;
pub mod press;
pub mod sensitivity;

use crate::nav::Page;

/// Something a page asks the app shell to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cmd {
    Goto(Page),
    /// Open the per-app editor of the profile at this index.
    OpenProfile(usize),
    OpenConfigFolder,
    DisableWindowsGestures,
    RestoreWindowsGestures,
    TogglePause,
}
