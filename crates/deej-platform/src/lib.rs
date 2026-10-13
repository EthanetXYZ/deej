//! Windows integration that isn't audio or serial: process names, the foreground app, app icons, starting with
//! Windows, sleep/resume notifications and the app's identity for notifications.
//! On other platforms these return nothing, so the rest of deej still builds and tests there.

pub mod app_id;
pub mod autostart;
pub mod foreground;
pub mod icon;
pub mod instance;
pub mod power;
pub mod process;
