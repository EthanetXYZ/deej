//! UI-only preferences (theme, window), kept in `deej-next-ui.json` next to `config.yaml` so the config file
//! stays readable by the Go deej (ARCHITECTURE.md D5).

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

pub const FILE_NAME: &str = "deej-next-ui.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    #[default]
    Dark,
    Light,
    System,
}

/// Where the window was when it was last closed, in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowState {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub maximized: bool,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UiPrefs {
    pub theme: Theme,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window: Option<WindowState>,
    /// The "deej keeps running in the tray" notification was shown (it's shown once).
    pub tray_hint_shown: bool,
    /// Only problems get a Windows notification (not "reconnected", "reloaded" or hints).
    pub quiet_notifications: bool,
}

/// What the UI can change. Fields left out stay as they are.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UiPrefsChange {
    pub theme: Option<Theme>,
    pub quiet_notifications: Option<bool>,
}

impl UiPrefsChange {
    pub fn apply(self, prefs: &mut UiPrefs) {
        if let Some(theme) = self.theme {
            prefs.theme = theme;
        }
        if let Some(quiet) = self.quiet_notifications {
            prefs.quiet_notifications = quiet;
        }
    }
}

pub struct PrefsFile {
    path: PathBuf,
    /// Serialises load-modify-save between the window (main thread) and commands.
    lock: Mutex<()>,
}

impl PrefsFile {
    pub fn new(dir: &Path) -> PrefsFile {
        PrefsFile { path: dir.join(FILE_NAME), lock: Mutex::new(()) }
    }

    /// The saved preferences; defaults if the file is missing or unreadable.
    pub fn load(&self) -> UiPrefs {
        std::fs::read_to_string(&self.path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    /// Changes some preferences, keeping the others.
    pub fn update(&self, change: impl FnOnce(&mut UiPrefs)) -> std::io::Result<()> {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut prefs = self.load();
        change(&mut prefs);
        self.save(&prefs)
    }

    fn save(&self, prefs: &UiPrefs) -> std::io::Result<()> {
        let json = serde_json::to_string_pretty(prefs).map_err(std::io::Error::other)?;
        let temp = self.path.with_extension("json.tmp");
        std::fs::write(&temp, json)?;
        std::fs::rename(&temp, &self.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_tolerates_bad_files() {
        let dir = tempfile::tempdir().unwrap();
        let file = PrefsFile::new(dir.path());
        assert_eq!(file.load(), UiPrefs::default());

        file.update(|p| p.theme = Theme::Light).unwrap();
        assert_eq!(file.load().theme, Theme::Light);

        std::fs::write(dir.path().join(FILE_NAME), "{ not json").unwrap();
        assert_eq!(file.load(), UiPrefs::default());

        std::fs::write(dir.path().join(FILE_NAME), r#"{"theme":"system","future":1}"#).unwrap();
        assert_eq!(file.load().theme, Theme::System);
    }

    #[test]
    fn ui_changes_only_touch_what_they_name() {
        let mut prefs = UiPrefs { theme: Theme::Light, ..Default::default() };
        let change: UiPrefsChange = serde_json::from_str(r#"{"quietNotifications":true}"#).unwrap();
        change.apply(&mut prefs);
        assert_eq!((prefs.theme, prefs.quiet_notifications), (Theme::Light, true));

        let change: UiPrefsChange = serde_json::from_str(r#"{"theme":"system"}"#).unwrap();
        change.apply(&mut prefs);
        assert_eq!((prefs.theme, prefs.quiet_notifications), (Theme::System, true));
    }

    #[test]
    fn updates_keep_the_other_fields() {
        let dir = tempfile::tempdir().unwrap();
        let file = PrefsFile::new(dir.path());
        let window = WindowState { x: -1200, y: 40, width: 1100, height: 700, maximized: false };

        file.update(|p| p.window = Some(window)).unwrap();
        file.update(|p| p.theme = Theme::Light).unwrap();

        let prefs = file.load();
        assert_eq!(prefs.window, Some(window));
        assert_eq!(prefs.theme, Theme::Light);
    }
}
