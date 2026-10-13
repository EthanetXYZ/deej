//! Where deej's `config.yaml` (and its `logs/` folder) live.
//!
//! Order (COMPATIBILITY_MATRIX C1/C2):
//! 1. next to the executable (portable install, how deej has always been used)
//! 2. the working directory (how upstream deej looked for it)
//! 3. `%APPDATA%\deej` (installed under Program Files, which isn't writable)
//!
//! If none exists, the first writable choice is where a new default config is created.

use std::path::{Path, PathBuf};

pub const CONFIG_FILE_NAME: &str = "config.yaml";
pub const LOG_DIR_NAME: &str = "logs";
pub const PREFERENCES_FILE_NAME: &str = "preferences.yaml";

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum LocationKind {
    ExecutableDir,
    WorkingDir,
    AppData,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ConfigLocation {
    pub kind: LocationKind,
    /// Folder holding `config.yaml` and `logs/`.
    pub dir: PathBuf,
    /// Whether `config.yaml` already exists there.
    pub exists: bool,
}

impl ConfigLocation {
    pub fn config_path(&self) -> PathBuf {
        self.dir.join(CONFIG_FILE_NAME)
    }

    pub fn log_dir(&self) -> PathBuf {
        self.dir.join(LOG_DIR_NAME)
    }

    /// The original deej's secondary mapping file, `logs/preferences.yaml`.
    pub fn preferences_path(&self) -> PathBuf {
        self.log_dir().join(PREFERENCES_FILE_NAME)
    }
}

/// Picks the config location. `is_writable` decides where a new config may be created.
pub fn locate(
    exe_dir: &Path,
    working_dir: &Path,
    app_data_dir: Option<&Path>,
    is_writable: impl Fn(&Path) -> bool,
) -> ConfigLocation {
    let candidates = [
        Some((LocationKind::ExecutableDir, exe_dir.to_path_buf())),
        Some((LocationKind::WorkingDir, working_dir.to_path_buf())),
        app_data_dir.map(|dir| (LocationKind::AppData, dir.join("deej"))),
    ];

    if let Some((kind, dir)) = candidates.iter().flatten().find(|(_, dir)| dir.join(CONFIG_FILE_NAME).is_file()) {
        return ConfigLocation { kind: *kind, dir: dir.clone(), exists: true };
    }

    // nothing yet: next to the exe if we can write there (portable), otherwise AppData
    if is_writable(exe_dir) || app_data_dir.is_none() {
        return ConfigLocation { kind: LocationKind::ExecutableDir, dir: exe_dir.to_path_buf(), exists: false };
    }

    ConfigLocation {
        kind: LocationKind::AppData,
        dir: app_data_dir.map(|d| d.join("deej")).unwrap_or_default(),
        exists: false,
    }
}

/// Checks whether files can be created in `dir` by creating and removing a probe file.
pub fn dir_is_writable(dir: &Path) -> bool {
    let probe = dir.join(".deej-write-test");
    match std::fs::write(&probe, b"") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dirs() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let (exe, cwd, app) = (root.path().join("exe"), root.path().join("cwd"), root.path().join("appdata"));
        for d in [&exe, &cwd, &app] {
            std::fs::create_dir_all(d).unwrap();
        }
        (root, exe, cwd, app)
    }

    #[test]
    fn prefers_the_executable_folder_then_cwd_then_appdata() {
        let (_root, exe, cwd, app) = dirs();
        std::fs::create_dir_all(app.join("deej")).unwrap();
        std::fs::write(app.join("deej").join(CONFIG_FILE_NAME), "").unwrap();
        assert_eq!(locate(&exe, &cwd, Some(&app), |_| true).kind, LocationKind::AppData);

        std::fs::write(cwd.join(CONFIG_FILE_NAME), "").unwrap();
        assert_eq!(locate(&exe, &cwd, Some(&app), |_| true).kind, LocationKind::WorkingDir);

        std::fs::write(exe.join(CONFIG_FILE_NAME), "").unwrap();
        let found = locate(&exe, &cwd, Some(&app), |_| true);
        assert_eq!((found.kind, found.exists), (LocationKind::ExecutableDir, true));
    }

    #[test]
    fn new_config_goes_to_appdata_when_the_exe_folder_is_read_only() {
        let (_root, exe, cwd, app) = dirs();

        let portable = locate(&exe, &cwd, Some(&app), |_| true);
        assert_eq!((portable.kind, portable.exists), (LocationKind::ExecutableDir, false));

        let installed = locate(&exe, &cwd, Some(&app), |_| false);
        assert_eq!(installed.kind, LocationKind::AppData);
        assert_eq!(installed.config_path(), app.join("deej").join(CONFIG_FILE_NAME));
    }

    #[test]
    fn writability_probe() {
        let dir = tempfile::tempdir().unwrap();
        assert!(dir_is_writable(dir.path()));
        assert!(!dir_is_writable(&dir.path().join("missing")));
    }
}
