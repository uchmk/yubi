//! OS-side building blocks: window manipulation, process lookup, key
//! injection and master volume. Independent of the input/engine layers.
#![allow(dead_code)]

pub mod keys;
pub mod process;
pub mod volume;
pub mod window;

/// Marker placed in `dwExtraInfo` of every input event yubi injects, so that
/// hooks / Raw Input handlers can recognise (and ignore) our own events.
pub const YUBI_INJECT_MARK: usize = 0x5955_4249;
