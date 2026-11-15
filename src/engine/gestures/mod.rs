//! The gesture state machines. Each one reads a [`Ctx`](super::types::Ctx), may propose to take
//! over the session (the arbiter decides) and, once it owns it, emits its actions.

pub mod drag;
pub mod edge;
pub mod multi;
pub mod press;
pub mod ring;
pub mod window;
