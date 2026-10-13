//! Audio control for deej Next.
//!
//! - [`model`]: the plain data the rest of deej works with (endpoints, sessions, commands, events)
//! - [`fake`]: an in-memory backend for tests
//! - `AudioEngine` (Windows only): the real Core Audio engine on its own thread
//!
//! See docs/ARCHITECTURE.md §3.2 for the design.

pub mod fake;

/// The audio types live in `deej-core` so the mapping logic can use them without depending on this crate.
pub use deej_core::audio as model;

#[cfg(windows)]
mod windows;

#[cfg(windows)]
pub use windows::{AudioEngine, DEEJ_EVENT_CONTEXT, DEFAULT_RECONCILE_INTERVAL};

pub use model::*;
