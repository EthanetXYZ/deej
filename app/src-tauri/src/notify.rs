//! Windows notifications, for things worth knowing while the window is closed: the board dropping out, a broken
//! config.yaml, and where deej went after its window was closed.

use tauri::{AppHandle, Manager};
use tauri_plugin_notification::NotificationExt;

use crate::AppState;
use crate::coordinator::UiEvent;

/// The AppUserModelID notifications are shown under (the bundle identifier, registered at startup).
pub const APP_ID: &str = "com.ethanetxyz.deej";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Something is wrong (board gone, config broken): always shown.
    Problem,
    /// Good news and hints (reconnected, reloaded): can be turned off in Settings.
    Routine,
}

pub fn show(app: &AppHandle, kind: Kind, title: &str, body: &str) {
    if kind == Kind::Routine && app.state::<AppState>().prefs.load().quiet_notifications {
        tracing::info!(%title, "Notification skipped (routine notifications are off)");
        return;
    }
    tracing::info!(%title, %body, "Notification");
    if let Err(err) = app.notification().builder().title(title).body(body).show() {
        tracing::warn!(error = %err, "Couldn't show a notification");
    }
}

/// Config events get a notification when there's no window to show them in (the window has its own toasts).
pub fn background_event(app: &AppHandle, event: &UiEvent) {
    match event {
        UiEvent::ConfigError { message } => show(
            app,
            Kind::Problem,
            "config.yaml has a problem",
            &format!("The previous settings stay active. {message}"),
        ),
        UiEvent::ConfigReloaded { .. } => {
            show(app, Kind::Routine, "Configuration reloaded", "deej is using the new config.yaml.")
        }
        _ => {}
    }
}
