//! Finding and loading `config.yaml` at startup.
//!
//! Unlike the Go deej, startup never fails on configuration (COMPATIBILITY_MATRIX C3/C4): a missing file is
//! created from the default, and an invalid one leaves the built-in defaults active and the error on screen
//! until the file is fixed.

use std::fs;
use std::path::{Path, PathBuf};

use deej_core::config::{self, Config, DEFAULT_CONFIG_YAML, Diagnostic, Severity};
use deej_core::location::{self, ConfigLocation};
use serde::Serialize;

/// The outcome of loading the config, as shown to the user.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigStatus {
    pub path: PathBuf,
    pub location_kind: location::LocationKind,
    pub created_default: bool,
    /// Why the file couldn't be used, if it couldn't. The defaults are active in that case.
    pub error: Option<String>,
    pub diagnostics: Vec<Diagnostic>,
    pub slider_count: usize,
    pub com_port: String,
    pub baud_rate: u32,
}

pub struct StartupConfig {
    pub location: ConfigLocation,
    pub config: Config,
    pub status: ConfigStatus,
}

/// Where the config lives, based on the running executable, the working directory and `%APPDATA%`.
pub fn locate() -> ConfigLocation {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."));
    let working_dir = std::env::current_dir().unwrap_or_else(|_| exe_dir.clone());
    let app_data = std::env::var_os("APPDATA").map(PathBuf::from);

    location::locate(&exe_dir, &working_dir, app_data.as_deref(), location::dir_is_writable)
}

/// Loads (or creates) the config at `location`. Never fails; problems end up in [`ConfigStatus::error`].
pub fn load(location: ConfigLocation) -> StartupConfig {
    let path = location.config_path();
    let mut created_default = false;

    if !location.exists {
        match create_default(&location) {
            Ok(()) => {
                tracing::info!(path = %path.display(), "No config found, created the default one");
                created_default = true;
            }
            Err(err) => tracing::error!(path = %path.display(), error = %err, "Couldn't create the default config"),
        }
    }

    let preferences = fs::read_to_string(location.preferences_path()).ok();

    let (config, diagnostics, error) = match fs::read_to_string(&path) {
        Err(err) => (Config::default(), Vec::new(), Some(format!("Couldn't read {}: {err}", path.display()))),
        Ok(source) => match config::parse(&source, preferences.as_deref()) {
            Ok(loaded) => (loaded.config, loaded.diagnostics, None),
            Err(err) => (Config::default(), Vec::new(), Some(err.to_string())),
        },
    };

    match &error {
        None => tracing::info!(
            path = %path.display(),
            sliders = config.slider_mapping.len(),
            com_port = %config.com_port,
            baud_rate = config.baud_rate,
            invert = config.invert_sliders,
            noise = config.noise_reduction.as_str(),
            "Loaded config"
        ),
        Some(err) => tracing::warn!(path = %path.display(), error = %err, "Config unusable, running with defaults"),
    }

    for d in &diagnostics {
        match d.severity {
            Severity::Warning => tracing::warn!(key = %d.key, line = ?d.line, "{}", d.message),
            Severity::Info => tracing::info!(key = %d.key, line = ?d.line, "{}", d.message),
        }
    }

    let status = ConfigStatus {
        path,
        location_kind: location.kind,
        created_default,
        error,
        diagnostics,
        slider_count: config.slider_mapping.len(),
        com_port: config.com_port.clone(),
        baud_rate: config.baud_rate,
    };

    StartupConfig { location, config, status }
}

fn create_default(location: &ConfigLocation) -> std::io::Result<()> {
    fs::create_dir_all(&location.dir)?;

    // create_new: never overwrite a config that appeared in the meantime
    use std::io::Write;
    let mut file = fs::OpenOptions::new().write(true).create_new(true).open(location.config_path())?;
    file.write_all(DEFAULT_CONFIG_YAML.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use deej_core::location::LocationKind;

    fn location_in(dir: &Path, exists: bool) -> ConfigLocation {
        ConfigLocation { kind: LocationKind::ExecutableDir, dir: dir.to_path_buf(), exists }
    }

    #[test]
    fn missing_config_is_created_from_the_default() {
        let dir = tempfile::tempdir().unwrap();
        let started = load(location_in(dir.path(), false));

        assert!(started.status.created_default);
        assert!(started.status.error.is_none());
        assert_eq!(fs::read_to_string(dir.path().join("config.yaml")).unwrap(), DEFAULT_CONFIG_YAML);
        assert_eq!(started.status.slider_count, 5);
    }

    #[test]
    fn invalid_config_falls_back_to_defaults_and_reports_why() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("config.yaml"), "slider_mapping: [\n").unwrap();

        let started = load(location_in(dir.path(), true));
        assert!(started.status.error.as_deref().unwrap().contains("isn't valid YAML"));
        assert_eq!(started.config, Config::default());
    }

    #[test]
    fn preferences_file_in_logs_is_merged() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("config.yaml"), "slider_mapping:\n  0: master\n").unwrap();
        fs::create_dir_all(dir.path().join("logs")).unwrap();
        fs::write(dir.path().join("logs/preferences.yaml"), "slider_mapping:\n  1: chrome.exe\n").unwrap();

        let started = load(location_in(dir.path(), true));
        assert_eq!(started.config.slider_mapping.len(), 2);
    }
}
