//! Platform-independent core of deej Next.
//!
//! Nothing in this crate touches Windows, audio devices or serial ports, so all of it is unit-testable on any
//! machine. See `docs/ARCHITECTURE.md` for how it fits together.

pub mod audio;
pub mod config;
pub mod location;
pub mod mapping;
pub mod protocol;
pub mod yaml;
