//! The tray icon: deej's home while the window is closed.
//!
//! Left-click opens the window; the menu has the board status, master mute, the playback device, the fork's
//! actions (restart the board connection, re-scan, edit config.yaml), Start with Windows, the version and Quit. The
//! tooltip is just "deej" (owner decision D27: the fork's per-slider tooltip isn't needed).
//!
//! A `deej-tray` thread refreshes the menu twice a second from the coordinator's published state, and
//! only touches the tray when something visible changed. It also raises one notification per board outage.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use deej_core::audio::{EndpointRef, Flow, VolumeTarget};
use deej_serial::{ConnectionState, Status};
use tauri::menu::{CheckMenuItem, IsMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Wry};

use crate::coordinator::{Msg, Shared};
use crate::{AppState, notify, window};

const REFRESH_INTERVAL: Duration = Duration::from_millis(500);

/// A board problem is only announced if it lasts this long (USB re-enumeration at sign-in takes a moment).
const OUTAGE_NOTICE_AFTER: Duration = Duration::from_secs(5);

const ID_OPEN: &str = "open";
const ID_MUTE: &str = "mute";
const ID_DEVICE_PREFIX: &str = "device:";
const ID_RECONNECT: &str = "reconnect";
const ID_RESCAN: &str = "rescan";
const ID_EDIT: &str = "edit";
const ID_AUTOSTART: &str = "autostart";
const ID_QUIT: &str = "quit";

/// Everything the menu shows; the menu is rebuilt only when this changes.
#[derive(Debug, Clone, PartialEq, Default)]
struct MenuModel {
    status: String,
    /// Playback devices: (id, name, is default).
    devices: Vec<(String, String, bool)>,
    /// Mute state of the default playback device, if there is one.
    master_muted: Option<bool>,
    autostart: bool,
}

pub fn create(app: &AppHandle) -> tauri::Result<TrayIcon> {
    let mut builder = TrayIconBuilder::with_id("main")
        .tooltip("deej")
        .show_menu_on_left_click(false)
        .menu(&build_menu(app, &MenuModel::default())?)
        .on_menu_event(|app, event| on_menu(app, event.id.as_ref()))
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                window::show(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)
}

/// Keeps the tray up to date until `stop` is set.
pub fn start_refresher(app: AppHandle, tray: TrayIcon, stop: Arc<AtomicBool>) {
    let spawned = std::thread::Builder::new().name("deej-tray".into()).spawn(move || {
        let mut menu = MenuModel::default();
        let mut outage = Outage::default();

        while !stop.load(Ordering::Relaxed) {
            let state = app.state::<AppState>();
            let (model, hardware) = {
                let shared = state.shared.lock().unwrap_or_else(|e| e.into_inner());
                let autostart = state.autostart_enabled.load(Ordering::Relaxed);
                (menu_model(&shared, autostart), shared.hardware.clone())
            };

            if model != menu {
                match build_menu(&app, &model) {
                    Ok(built) => {
                        let _ = tray.set_menu(Some(built));
                        menu = model;
                    }
                    Err(err) => tracing::warn!(error = %err, "Couldn't update the tray menu"),
                }
            }
            if let Some((kind, title, body)) = outage.update(hardware.as_ref(), Instant::now()) {
                notify::show(&app, kind, title, &body);
            }

            std::thread::sleep(REFRESH_INTERVAL);
        }
    });
    if let Err(err) = spawned {
        tracing::error!(error = %err, "Couldn't start the tray refresher");
    }
}

fn on_menu(app: &AppHandle, id: &str) {
    let state = app.state::<AppState>();
    let send = |msg: Msg| {
        if let Some(services) = state.services.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            services.send(msg);
        }
    };

    match id {
        ID_OPEN => window::show(app),
        ID_MUTE => {
            let muted = state
                .shared
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .audio
                .default_endpoint(Flow::Render)
                .map(|e| e.muted);
            if let Some(muted) = muted {
                send(Msg::SetMute {
                    target: VolumeTarget::Endpoint(EndpointRef::Default(Flow::Render)),
                    muted: !muted,
                });
            }
        }
        ID_RECONNECT => send(Msg::ReconnectHardware),
        ID_RESCAN => send(Msg::Rescan),
        ID_EDIT => {
            if let Err(err) = std::process::Command::new("notepad.exe").arg(state.location.config_path()).spawn() {
                tracing::warn!(error = %err, "Couldn't open config.yaml in Notepad");
            }
        }
        ID_AUTOSTART => {
            let enable = !state.autostart_enabled.load(Ordering::Relaxed);
            let app = app.clone();
            // registry and shortcut work stays off the main thread
            std::thread::spawn(move || {
                if let Err(err) = crate::set_autostart(&app, enable) {
                    tracing::warn!(error = %err, "Couldn't change Start with Windows");
                    notify::show(&app, notify::Kind::Problem, "Couldn't change Start with Windows", &err.to_string());
                }
            });
        }
        ID_QUIT => {
            tracing::info!("Quit from the tray");
            window::save_state(app);
            app.exit(0);
        }
        other => {
            if let Some(device) = other.strip_prefix(ID_DEVICE_PREFIX) {
                send(Msg::SetDefaultEndpoint(device.to_string()));
            }
        }
    }
}

fn menu_model(shared: &Shared, autostart: bool) -> MenuModel {
    let mut devices: Vec<(String, String, bool)> = shared
        .audio
        .endpoints
        .iter()
        .filter(|e| e.flow == Flow::Render)
        .map(|e| (e.id.clone(), e.name.clone(), e.is_default))
        .collect();
    devices.sort_by_key(|d| d.1.to_lowercase());

    MenuModel {
        status: status_line(shared.hardware.as_ref()),
        devices,
        master_muted: shared.audio.default_endpoint(Flow::Render).map(|e| e.muted),
        autostart,
    }
}

fn build_menu(app: &AppHandle, model: &MenuModel) -> tauri::Result<Menu<Wry>> {
    let status = MenuItem::with_id(app, "status", &model.status, false, None::<&str>)?;
    let open = MenuItem::with_id(app, ID_OPEN, "Open deej", true, None::<&str>)?;
    let mute = CheckMenuItem::with_id(
        app,
        ID_MUTE,
        "Mute playback",
        model.master_muted.is_some(),
        model.master_muted.unwrap_or(false),
        None::<&str>,
    )?;

    let device_items = model
        .devices
        .iter()
        .map(|(id, name, is_default)| {
            CheckMenuItem::with_id(
                app,
                format!("{ID_DEVICE_PREFIX}{id}"),
                escape(name),
                true,
                *is_default,
                None::<&str>,
            )
        })
        .collect::<tauri::Result<Vec<_>>>()?;
    let device_refs: Vec<&dyn IsMenuItem<Wry>> = device_items.iter().map(|i| i as &dyn IsMenuItem<Wry>).collect();
    let devices = Submenu::with_items(app, "Playback device", !device_items.is_empty(), &device_refs)?;

    let reconnect = MenuItem::with_id(app, ID_RECONNECT, "Restart board connection", true, None::<&str>)?;
    let rescan = MenuItem::with_id(app, ID_RESCAN, "Re-scan audio", true, None::<&str>)?;
    let edit = MenuItem::with_id(app, ID_EDIT, "Edit config.yaml", true, None::<&str>)?;
    let autostart =
        CheckMenuItem::with_id(app, ID_AUTOSTART, "Start with Windows", true, model.autostart, None::<&str>)?;
    let version = MenuItem::with_id(app, "version", format!("deej {}", crate::version_string()), false, None::<&str>)?;
    let quit = MenuItem::with_id(app, ID_QUIT, "Quit deej", true, None::<&str>)?;

    Menu::with_items(
        app,
        &[
            &status,
            &open,
            &PredefinedMenuItem::separator(app)?,
            &mute,
            &devices,
            &PredefinedMenuItem::separator(app)?,
            &reconnect,
            &rescan,
            &edit,
            &autostart,
            &PredefinedMenuItem::separator(app)?,
            &version,
            &quit,
        ],
    )
}

/// `&` marks a mnemonic in Windows menus; device names like "Speakers (R&D)" need it doubled.
fn escape(text: &str) -> String {
    text.replace('&', "&&")
}

pub fn status_line(status: Option<&Status>) -> String {
    let Some(s) = status else { return "Starting…".into() };
    match &s.state {
        ConnectionState::Connected => format!("Connected ({})", s.port),
        ConnectionState::Connecting => format!("Connecting to {}…", s.port),
        ConnectionState::NotFound => format!("{} not found, retrying…", s.port),
        ConnectionState::Busy => format!("{} is in use by another program", s.port),
        ConnectionState::Lost => format!("Lost {}, retrying…", s.port),
        ConnectionState::Failed { message } => format!("{}: {message}", s.port),
    }
}

// ---- board outage notifications -------------------------------------------------------------------------

/// Tracks board outages so each one is announced once (like the fork's one toast per outage), and its end only if
/// its start was announced.
#[derive(Debug, Default)]
struct Outage {
    since: Option<Instant>,
    announced: bool,
}

impl Outage {
    fn update(&mut self, status: Option<&Status>, now: Instant) -> Option<(notify::Kind, &'static str, String)> {
        let status = status?;
        match &status.state {
            ConnectionState::Connected => {
                let was_announced = std::mem::take(&mut self.announced);
                self.since = None;
                was_announced.then(|| {
                    (
                        notify::Kind::Routine,
                        "Board reconnected",
                        format!("deej is reading the sliders on {} again.", status.port),
                    )
                })
            }
            ConnectionState::Connecting => None,
            problem => {
                let since = *self.since.get_or_insert(now);
                if self.announced || now.duration_since(since) < OUTAGE_NOTICE_AFTER {
                    return None;
                }
                self.announced = true;
                let body = match problem {
                    ConnectionState::Busy => format!(
                        "{} is in use by another program (another deej, or a serial monitor). deej keeps retrying.",
                        status.port
                    ),
                    ConnectionState::Failed { message } => {
                        format!("Can't open {}: {message}. deej keeps retrying.", status.port)
                    }
                    _ => format!(
                        "No board on {}. deej keeps retrying, and reconnects when it's plugged in.",
                        status.port
                    ),
                };
                Some((notify::Kind::Problem, "Board disconnected", body))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_outage_is_announced_once_after_a_grace_period() {
        let status = |state| Status { port: "COM3".into(), baud_rate: 9600, state };
        let t0 = Instant::now();
        let mut outage = Outage::default();

        assert_eq!(outage.update(Some(&status(ConnectionState::Connected)), t0), None);
        assert_eq!(outage.update(Some(&status(ConnectionState::NotFound)), t0), None, "grace period");
        let (_, title, _) = outage.update(Some(&status(ConnectionState::NotFound)), t0 + OUTAGE_NOTICE_AFTER).unwrap();
        assert_eq!(title, "Board disconnected");
        assert_eq!(outage.update(Some(&status(ConnectionState::Lost)), t0 + OUTAGE_NOTICE_AFTER * 3), None, "once");
        let (_, title, _) =
            outage.update(Some(&status(ConnectionState::Connected)), t0 + OUTAGE_NOTICE_AFTER * 4).unwrap();
        assert_eq!(title, "Board reconnected");

        // a short blip is never mentioned
        assert_eq!(outage.update(Some(&status(ConnectionState::Lost)), t0 + OUTAGE_NOTICE_AFTER * 5), None);
        assert_eq!(outage.update(Some(&status(ConnectionState::Connected)), t0 + OUTAGE_NOTICE_AFTER * 5), None);
    }
}
