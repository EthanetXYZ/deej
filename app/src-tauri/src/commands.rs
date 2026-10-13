//! Tauri commands: the only way the UI talks to the backend. Their TypeScript counterparts are in `app/src/ipc.ts`.
//!
//! Commands that change settings validate their input here, then hand an edit to the coordinator, which applies it
//! immediately and saves `config.yaml` shortly after. Only the keys listed in [`set_setting`] can be changed.

use std::collections::HashMap;
use std::sync::Mutex;

use deej_core::config::sensitivity::{MAX_CURVE, MIN_CURVE};
use deej_core::config::writer::{Edit, Value};
use deej_core::config::{CURVE_PRESETS, Config, CurvePreset};
use deej_serial::{Diagnostics, PortInfo};
use serde::Serialize;
use tauri::State;

use crate::AppState;
use crate::coordinator::{Msg, Shared};
use crate::prefs::{UiPrefs, UiPrefsChange};
use crate::startup::ConfigStatus;

/// An error returned to the UI. Serialized as its message.
#[derive(Debug, thiserror::Error)]
pub enum CommandError {
    #[error("internal state is unavailable")]
    StatePoisoned,
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    Failed(String),
}

impl Serialize for CommandError {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

type Result<T> = std::result::Result<T, CommandError>;

fn invalid<T>(message: impl Into<String>) -> Result<T> {
    Err(CommandError::Invalid(message.into()))
}

fn send(state: &State<'_, AppState>, msg: Msg) -> Result<()> {
    let services = state.services.lock().map_err(|_| CommandError::StatePoisoned)?;
    if let Some(services) = services.as_ref() {
        services.send(msg);
    }
    Ok(())
}

fn edit(state: &State<'_, AppState>, edits: Vec<Edit>) -> Result<()> {
    send(state, Msg::Edit(edits))
}

// ---- reading state ---------------------------------------------------------------------------------------

#[tauri::command]
pub fn get_config(state: State<'_, AppState>) -> Result<Config> {
    state.shared.lock().map(|s| s.config.clone()).map_err(|_| CommandError::StatePoisoned)
}

#[tauri::command]
pub fn get_config_status(state: State<'_, AppState>) -> Result<ConfigStatus> {
    state.config_status.lock().map(|s| s.clone()).map_err(|_| CommandError::StatePoisoned)
}

/// Everything the UI shows: config, audio devices and apps, slider positions, board status.
#[tauri::command]
pub fn get_state(state: State<'_, AppState>) -> Result<Shared> {
    state.shared.lock().map(|s| s.clone()).map_err(|_| CommandError::StatePoisoned)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HardwareInfo {
    diagnostics: Option<Diagnostics>,
    ports: Vec<PortInfo>,
}

/// Serial ports on this machine and the board connection's counters.
#[tauri::command]
pub async fn get_hardware_info(state: State<'_, AppState>) -> Result<HardwareInfo> {
    let diagnostics =
        state.services.lock().map_err(|_| CommandError::StatePoisoned)?.as_ref().map(|s| s.serial_diagnostics());
    // listing ports queries the device registry; keep it off the UI thread
    let ports = tauri::async_runtime::spawn_blocking(deej_serial::available_ports).await.unwrap_or_default();
    Ok(HardwareInfo { diagnostics, ports })
}

#[tauri::command]
pub fn get_curve_presets() -> Vec<CurvePreset> {
    CURVE_PRESETS.to_vec()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    version: String,
    build_type: &'static str,
}

#[tauri::command]
pub fn get_app_info() -> AppInfo {
    AppInfo { version: crate::version_string(), build_type: crate::build_type() }
}

pub type IconCache = Mutex<HashMap<String, Option<String>>>;

/// Icon of an app's executable as a PNG data URL (cached).
#[tauri::command]
pub async fn get_app_icon(state: State<'_, AppState>, path: String) -> Result<Option<String>> {
    if let Some(cached) = state.icons.lock().map_err(|_| CommandError::StatePoisoned)?.get(&path) {
        return Ok(cached.clone());
    }

    let lookup = path.clone();
    let icon = tauri::async_runtime::spawn_blocking(move || with_com(|| deej_platform::icon::exe_icon_png(&lookup)))
        .await
        .unwrap_or(None);

    state.icons.lock().map_err(|_| CommandError::StatePoisoned)?.insert(path, icon.clone());
    Ok(icon)
}

/// Runs `f` with COM initialized on the current (worker) thread; the shell's icon lookup may need it.
#[cfg(windows)]
fn with_com<T>(f: impl FnOnce() -> T) -> T {
    use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};

    // SAFETY: a successful initialization is balanced by CoUninitialize on the same thread
    let initialized = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok() };
    let result = f();
    if initialized {
        // SAFETY: see above
        unsafe { CoUninitialize() };
    }
    result
}

#[cfg(not(windows))]
fn with_com<T>(f: impl FnOnce() -> T) -> T {
    f()
}

// ---- mappings and settings -------------------------------------------------------------------------------

const MAX_SLIDER: usize = 255;
const MAX_NAME: usize = 260;

fn check_name(name: &str) -> Result<()> {
    if name.trim().is_empty() || name.len() > MAX_NAME || name.chars().any(char::is_control) {
        return invalid(format!("{name:?} isn't a valid app or device name"));
    }
    Ok(())
}

/// What a slider controls. An empty list removes the slider from `slider_mapping`.
#[tauri::command]
pub fn set_slider_targets(state: State<'_, AppState>, slider: usize, targets: Vec<String>) -> Result<()> {
    if slider > MAX_SLIDER {
        return invalid("slider index out of range");
    }

    let mut unique: Vec<String> = Vec::new();
    for target in targets {
        let target = target.trim().to_string();
        check_name(&target)?;
        if !unique.iter().any(|t| t.eq_ignore_ascii_case(&target)) {
            unique.push(target);
        }
    }

    let index = slider.to_string();
    let path = ["slider_mapping", index.as_str()];
    let change = match unique.len() {
        0 => Edit::remove(&path),
        1 => Edit::set(&path, Value::Str(unique.remove(0))),
        _ => Edit::set(&path, Value::List(unique)),
    };
    edit(&state, vec![change])
}

/// Sets a slider's sensitivity fields, or with `clear`, makes it follow the default again. `slider: None` is the
/// default for all sliders. `max_volume` is 1–100.
#[tauri::command]
pub fn set_sensitivity(
    state: State<'_, AppState>,
    slider: Option<usize>,
    curve: Option<f64>,
    max_volume: Option<f64>,
    clear: bool,
) -> Result<()> {
    let section = match slider {
        Some(i) if i <= MAX_SLIDER => i.to_string(),
        Some(_) => return invalid("slider index out of range"),
        None => "default".to_string(),
    };

    if clear {
        return edit(&state, vec![Edit::remove(&["slider_sensitivity", &section])]);
    }

    let mut edits = Vec::new();
    if let Some(curve) = curve {
        if !(MIN_CURVE..=MAX_CURVE).contains(&curve) {
            return invalid(format!("curve must be between {MIN_CURVE} and {MAX_CURVE}"));
        }
        edits
            .push(Edit::set(&["slider_sensitivity", &section, "curve"], Value::Float((curve * 100.0).round() / 100.0)));
    }
    if let Some(max) = max_volume {
        if !(1.0..=100.0).contains(&max) {
            return invalid("max volume must be between 1 and 100");
        }
        edits.push(Edit::set(&["slider_sensitivity", &section, "max_volume"], Value::Int(max.round() as i64)));
    }
    edit(&state, edits)
}

/// Changes one of the general settings. Only these keys are accepted.
#[tauri::command]
pub fn set_setting(state: State<'_, AppState>, key: String, value: serde_json::Value) -> Result<()> {
    use serde_json::Value as Json;

    let new_value = match (key.as_str(), &value) {
        ("invert_sliders" | "deej_next.serial.dtr" | "deej_next.serial.rts", Json::Bool(b)) => Value::Bool(*b),
        ("noise_reduction", Json::String(s)) if matches!(s.as_str(), "low" | "default" | "high") => {
            Value::Str(s.clone())
        }
        ("com_port", Json::String(s)) if !s.trim().is_empty() && s.len() <= 64 => Value::Str(s.trim().into()),
        ("baud_rate", Json::Number(n)) if n.as_u64().is_some_and(|b| (300..=4_000_000).contains(&b)) => {
            Value::Int(n.as_i64().unwrap_or_default())
        }
        ("deej_next.serial.reconnect_interval_ms", Json::Number(n))
            if n.as_u64().is_some_and(|ms| (250..=60_000).contains(&ms)) =>
        {
            Value::Int(n.as_i64().unwrap_or_default())
        }
        _ => return invalid(format!("{key} can't be set to {value}")),
    };

    let path: Vec<&str> = key.split('.').collect();
    edit(&state, vec![Edit::set(&path, new_value)])
}

/// Apps `deej.unmapped` should leave alone (lowercased, de-duplicated).
#[tauri::command]
pub fn set_unmapped_exclude(state: State<'_, AppState>, names: Vec<String>) -> Result<()> {
    let mut list: Vec<String> = Vec::new();
    for name in names {
        let name = name.trim().to_lowercase();
        check_name(&name)?;
        if !list.contains(&name) {
            list.push(name);
        }
    }
    if list.len() > 200 {
        return invalid("too many apps");
    }
    edit(&state, vec![Edit::set(&["unmapped_exclude"], Value::List(list))])
}

// ---- actions ---------------------------------------------------------------------------------------------

/// Re-scans audio devices and apps, and re-applies every slider.
#[tauri::command]
pub fn rescan(state: State<'_, AppState>) -> Result<()> {
    send(&state, Msg::Rescan)
}

/// Closes and reopens the board's serial port.
#[tauri::command]
pub fn reconnect_hardware(state: State<'_, AppState>) -> Result<()> {
    send(&state, Msg::ReconnectHardware)
}

/// Opens config.yaml in Notepad, like the Go deej's "Edit configuration".
#[tauri::command]
pub fn open_config_file(state: State<'_, AppState>) -> Result<()> {
    let path = state.location.config_path();
    std::process::Command::new("notepad.exe").arg(path).spawn().map_err(|e| CommandError::Failed(e.to_string()))?;
    Ok(())
}

#[tauri::command]
pub fn open_logs_folder(state: State<'_, AppState>) -> Result<()> {
    let dir = state.location.log_dir();
    std::process::Command::new("explorer.exe").arg(dir).spawn().map_err(|e| CommandError::Failed(e.to_string()))?;
    Ok(())
}

// ---- UI diagnostics ------------------------------------------------------------------------------------

/// Lets the UI's own errors (which happen inside WebView2, out of sight) reach the log.
#[tauri::command]
pub fn report_ui_error(message: String) {
    let message: String = message.chars().take(2000).collect();
    tracing::error!(target: "deej_next::ui", %message, "UI error");
}

/// The UI's first render with real data, for the log (and for checking the window when it can't be seen).
#[tauri::command]
pub fn report_ui_ready(page: String, endpoints: usize, app_rows: usize, sliders: usize) {
    tracing::info!(target: "deej_next::ui", %page, endpoints, app_rows, sliders, "UI rendered");
}

// ---- UI preferences --------------------------------------------------------------------------------------

#[tauri::command]
pub fn get_ui_prefs(state: State<'_, AppState>) -> UiPrefs {
    state.prefs.load()
}

/// Saves the preferences the UI changed; the others (like the window position, which is the backend's) stay.
#[tauri::command]
pub fn set_ui_prefs(state: State<'_, AppState>, prefs: UiPrefsChange) -> Result<()> {
    state.prefs.update(|p| prefs.apply(p)).map_err(|e| CommandError::Failed(e.to_string()))
}

// ---- Start with Windows ----------------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Autostart {
    pub enabled: bool,
    pub via_shortcut: bool,
    /// Other deej programs that also start at sign-in.
    pub others: Vec<String>,
}

impl From<deej_platform::autostart::AutostartStatus> for Autostart {
    fn from(s: deej_platform::autostart::AutostartStatus) -> Autostart {
        Autostart { enabled: s.enabled, via_shortcut: s.via_shortcut, others: s.others }
    }
}

// async: reading shortcuts uses COM on a helper thread, kept off the window's thread
#[tauri::command(async)]
pub fn get_autostart(app: tauri::AppHandle) -> Result<Autostart> {
    Ok(crate::autostart_status(&app).into())
}

#[tauri::command(async)]
pub fn set_autostart(app: tauri::AppHandle, enabled: bool) -> Result<Autostart> {
    crate::set_autostart(&app, enabled).map(Into::into).map_err(|e| CommandError::Failed(e.to_string()))
}

#[tauri::command]
pub fn open_startup_folder() -> Result<()> {
    let dir = std::env::var_os("APPDATA")
        .map(|d| std::path::PathBuf::from(d).join(r"Microsoft\Windows\Start Menu\Programs\Startup"))
        .ok_or_else(|| CommandError::Failed("APPDATA isn't set".into()))?;
    std::process::Command::new("explorer.exe").arg(dir).spawn().map_err(|e| CommandError::Failed(e.to_string()))?;
    Ok(())
}
