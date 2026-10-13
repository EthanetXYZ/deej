// release builds have no console window, like the Go release build (-H=windowsgui)
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod coordinator;
mod logging;
mod notify;
mod prefs;
mod services;
mod startup;
mod tray;
mod window;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use deej_platform::autostart::{self, AutostartStatus, MINIMIZED_FLAG};
use deej_platform::power::{PowerEvent, PowerNotifications};
use tauri::{AppHandle, Emitter, Manager, RunEvent, WindowEvent};

/// Command-line options. Same flags as the Go deej, plus `--minimized`.
#[derive(Debug, Default, Clone)]
pub struct Args {
    /// `--verbose` / `-v`: debug-level logging (the Go build logged every serial line with it).
    pub verbose: bool,
    /// `--minimized`: start in the tray without opening the window (used by Start with Windows).
    pub minimized: bool,
}

impl Args {
    pub fn parse(args: impl IntoIterator<Item = String>) -> Args {
        let mut parsed = Args::default();
        for arg in args {
            match arg.as_str() {
                "--verbose" | "-v" => parsed.verbose = true,
                MINIMIZED_FLAG => parsed.minimized = true,
                // the Go deej's flag for opening its settings window; the window opens by default here
                "--settings" => parsed.minimized = false,
                _ => {}
            }
        }
        parsed
    }
}

pub fn build_type() -> &'static str {
    if cfg!(debug_assertions) { "dev" } else { "release" }
}

/// e.g. `0.1.0 (release, 7f23905)`
pub fn version_string() -> String {
    let commit = env!("DEEJ_GIT_COMMIT");
    if commit.is_empty() {
        format!("{} ({})", env!("CARGO_PKG_VERSION"), build_type())
    } else {
        format!("{} ({}, {commit})", env!("CARGO_PKG_VERSION"), build_type())
    }
}

/// Shared state reachable from Tauri commands.
pub struct AppState {
    /// What the coordinator last published: config, audio, slider positions, hardware status.
    pub shared: Arc<Mutex<coordinator::Shared>>,
    /// How the config was found and loaded at startup.
    pub config_status: Mutex<startup::ConfigStatus>,
    /// The background work; `None` once stopped.
    pub services: Mutex<Option<services::Services>>,
    /// Where config.yaml and logs/ are.
    pub location: deej_core::location::ConfigLocation,
    pub prefs: prefs::PrefsFile,
    pub icons: commands::IconCache,
    /// Whether this exe starts with Windows, as last read (shown in the tray menu).
    pub autostart_enabled: AtomicBool,
    /// False with `DEEJ_NO_TRAY_ICON` set (like the Go deej); closing the window then quits.
    pub tray_enabled: bool,
    /// Stops the tray refresher at exit.
    pub tray_stop: Arc<AtomicBool>,
    /// Sleep/resume registration, kept alive until exit.
    pub power: Mutex<Option<PowerNotifications>>,
}

/// Name of the Tauri event carrying every `coordinator::UiEvent`.
pub const UI_EVENT: &str = "deej://event";

fn current_exe() -> std::io::Result<std::path::PathBuf> {
    std::env::current_exe()
}

/// Reads whether deej starts with Windows (and remembers it for the tray).
pub fn autostart_status(app: &AppHandle) -> AutostartStatus {
    let status = current_exe().map(|exe| autostart::status(&exe)).unwrap_or_default();
    app.state::<AppState>().autostart_enabled.store(status.enabled, Ordering::Relaxed);
    status
}

pub fn set_autostart(app: &AppHandle, enabled: bool) -> std::io::Result<AutostartStatus> {
    let exe = current_exe()?;
    autostart::set(&exe, enabled)?;
    tracing::info!(enabled, exe = %exe.display(), "Start with Windows changed");
    Ok(autostart_status(app))
}

/// Gives notifications deej's name and icon, also when it isn't installed (ARCHITECTURE.md R10).
fn register_app_id(app: &AppHandle) {
    let icon = app.path().app_local_data_dir().ok().and_then(|dir| {
        let path = dir.join("notification-icon.png");
        std::fs::create_dir_all(&dir).ok()?;
        std::fs::write(&path, include_bytes!("../icons/128x128.png")).ok()?;
        Some(path)
    });
    if let Err(err) = deej_platform::app_id::register(notify::APP_ID, "deej", icon.as_deref()) {
        tracing::warn!(error = %err, "Couldn't register deej's notification identity");
    }
}

fn main() {
    let args = Args::parse(std::env::args().skip(1));
    let location = startup::locate();

    // a second copy only hands its launch to the running one (single-instance plugin below) and must leave that
    // copy's log files alone
    let instance = deej_platform::instance::acquire(notify::APP_ID);
    let _log_guard = logging::init(&location.log_dir(), args.verbose, instance.is_some());
    tracing::info!(version = %version_string(), ?args, "deej starting");

    let started = startup::load(location);
    let shared = Arc::new(Mutex::new(coordinator::Shared {
        config: started.config.clone(),
        config_error: started.status.error.clone(),
        diagnostics: started.status.diagnostics.clone(),
        ..Default::default()
    }));
    let state = AppState {
        shared: shared.clone(),
        config_status: Mutex::new(started.status),
        services: Mutex::new(None),
        location: started.location.clone(),
        prefs: prefs::PrefsFile::new(&started.location.dir),
        icons: Default::default(),
        autostart_enabled: AtomicBool::new(false),
        tray_enabled: std::env::var_os("DEEJ_NO_TRAY_ICON").is_none(),
        tray_stop: Arc::new(AtomicBool::new(false)),
        power: Mutex::new(None),
    };

    let app = tauri::Builder::default()
        // first, so a second launch exits before starting anything (and opens this one's window instead)
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            tracing::info!(?argv, "deej was started again");
            if !argv.iter().any(|a| a == MINIMIZED_FLAG) {
                window::show(app);
            }
        }))
        .plugin(tauri_plugin_notification::init())
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            commands::get_config,
            commands::get_config_status,
            commands::get_state,
            commands::get_hardware_info,
            commands::get_curve_presets,
            commands::get_app_info,
            commands::get_app_icon,
            commands::set_slider_targets,
            commands::set_sensitivity,
            commands::set_setting,
            commands::set_unmapped_exclude,
            commands::rescan,
            commands::reconnect_hardware,
            commands::open_config_file,
            commands::open_logs_folder,
            commands::report_ui_error,
            commands::report_ui_ready,
            commands::get_ui_prefs,
            commands::set_ui_prefs,
            commands::get_autostart,
            commands::set_autostart,
            commands::open_startup_folder
        ])
        .setup(move |app| {
            let handle = app.handle().clone();
            register_app_id(&handle);

            let ui_handle = handle.clone();
            let ui = Box::new(move |event: coordinator::UiEvent| {
                if ui_handle.get_webview_window(window::MAIN).is_none() {
                    notify::background_event(&ui_handle, &event);
                }
                let _ = ui_handle.emit(UI_EVENT, event);
            });
            let services = services::Services::start(&started.location, started.config, shared, ui)?;
            let state = app.state::<AppState>();
            *state.services.lock().unwrap() = Some(services);

            if state.tray_enabled {
                let tray = tray::create(&handle)?;
                tray::start_refresher(handle.clone(), tray, state.tray_stop.clone());
            } else {
                tracing::info!("DEEJ_NO_TRAY_ICON is set: no tray icon, and closing the window quits");
            }

            let power_handle = handle.clone();
            let power = PowerNotifications::register(Box::new(move |event| {
                tracing::info!(?event, "Power event");
                if event == PowerEvent::Resumed {
                    if let Some(services) = power_handle.state::<AppState>().services.lock().unwrap().as_ref() {
                        services.send(coordinator::Msg::Resumed);
                    }
                }
            }));
            match power {
                Ok(power) => *state.power.lock().unwrap() = Some(power),
                Err(err) => tracing::warn!(error = %err, "Can't watch for sleep/resume"),
            }

            // registry and Startup-shortcut reads stay off the main thread
            let autostart_handle = handle.clone();
            std::thread::spawn(move || {
                if let Ok(exe) = current_exe() {
                    match autostart::refresh(&exe) {
                        Ok(true) => tracing::info!(exe = %exe.display(), "Start with Windows now points at this deej"),
                        Ok(false) => {}
                        Err(err) => tracing::warn!(error = %err, "Couldn't update Start with Windows"),
                    }
                }
                let status = autostart_status(&autostart_handle);
                tracing::info!(enabled = status.enabled, others = ?status.others, "Start with Windows");
            });

            if !args.minimized {
                window::show(&handle);
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() == window::MAIN && matches!(event, WindowEvent::CloseRequested { .. }) {
                let app = window.app_handle();
                window::save_state(app);
                let state = app.state::<AppState>();
                if !state.tray_enabled {
                    return;
                }
                tracing::info!("Window closed; deej keeps running in the tray");
                if !state.prefs.load().tray_hint_shown {
                    notify::show(
                        app,
                        notify::Kind::Routine,
                        "deej is still running",
                        "Your sliders keep working. Click the deej icon in the tray to open it again; right-click it to quit.",
                    );
                    let _ = state.prefs.update(|p| p.tray_hint_shown = true);
                }
            }
        })
        .build(tauri::generate_context!());

    let app = match app {
        Ok(app) => app,
        Err(err) => {
            tracing::error!(error = %err, "Tauri failed to start");
            return;
        }
    };

    app.run(|app, event| match event {
        // closing the window must not end deej; only Quit (app.exit) does. Without a tray there'd be no way back,
        // so then closing the window quits.
        RunEvent::ExitRequested { code: None, api, .. } if app.state::<AppState>().tray_enabled => api.prevent_exit(),
        RunEvent::Exit => {
            let state = app.state::<AppState>();
            state.tray_stop.store(true, Ordering::Relaxed);
            state.power.lock().unwrap().take();
            // save pending edits and release the port and audio objects before the process ends
            if let Some(mut services) = state.services.lock().unwrap().take() {
                services.stop();
            }
            tracing::info!("deej stopped");
        }
        _ => {}
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags() {
        let parse = |args: &[&str]| Args::parse(args.iter().map(|a| a.to_string()));
        assert!(parse(&["--verbose"]).verbose);
        assert!(parse(&["-v"]).verbose);
        assert!(!parse(&["--other"]).verbose);
        assert!(parse(&["--minimized"]).minimized);
        assert!(!parse(&["--minimized", "--settings"]).minimized);
        assert!(!parse(&[]).minimized);
    }
}
