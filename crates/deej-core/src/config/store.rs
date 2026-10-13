//! Saving `config.yaml`: atomic writes, a backup before the first change of each run, and recognising our own
//! writes so the file watcher doesn't treat them as the user's edits.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use super::writer::{Edit, WriteError, apply_edits};

/// How many backups to keep.
pub const BACKUPS_KEPT: usize = 5;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("couldn't save config.yaml: {0}")]
    Io(#[from] io::Error),
    #[error(transparent)]
    Edit(#[from] WriteError),
}

pub struct ConfigStore {
    path: PathBuf,
    backup_dir: PathBuf,
    backed_up: bool,
    last_written: Option<String>,
}

impl ConfigStore {
    pub fn new(path: PathBuf, backup_dir: PathBuf) -> ConfigStore {
        ConfigStore { path, backup_dir, backed_up: false, last_written: None }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn read(&self) -> io::Result<String> {
        fs::read_to_string(&self.path)
    }

    /// Applies edits to the file as it is on disk now, and saves it. Returns the new contents.
    pub fn apply(&mut self, edits: &[Edit]) -> Result<String, StoreError> {
        let current = self.read()?;
        let updated = apply_edits(&current, edits)?;

        if updated == current {
            return Ok(updated);
        }

        if !self.backed_up {
            self.backup(&current)?;
            self.backed_up = true;
        }

        write_atomically(&self.path, &updated)?;
        self.last_written = Some(updated.clone());
        Ok(updated)
    }

    /// True if the file still holds exactly what deej last wrote (so a change notification was our own).
    pub fn is_own_write(&self, contents: &str) -> bool {
        self.last_written.as_deref() == Some(contents)
    }

    fn backup(&self, contents: &str) -> io::Result<()> {
        fs::create_dir_all(&self.backup_dir)?;
        let name = format!("config-{}.yaml", timestamp(SystemTime::now()));
        fs::write(self.backup_dir.join(name), contents)?;

        let mut backups: Vec<PathBuf> = fs::read_dir(&self.backup_dir)?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("config-")))
            .collect();
        backups.sort();

        let excess = backups.len().saturating_sub(BACKUPS_KEPT);
        for old in &backups[..excess] {
            let _ = fs::remove_file(old);
        }

        Ok(())
    }
}

/// Writes to a temporary file next to `path`, then renames it over `path`, so a crash can't leave half a file.
pub fn write_atomically(path: &Path, contents: &str) -> io::Result<()> {
    let temp = path.with_extension("yaml.tmp");
    fs::write(&temp, contents)?;

    if let Err(err) = fs::rename(&temp, path) {
        let _ = fs::remove_file(&temp);
        return Err(err);
    }

    Ok(())
}

/// `YYYYMMDD-HHMMSS` in UTC.
fn timestamp(time: SystemTime) -> String {
    let secs = time.duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let (days, rem) = ((secs / 86_400) as i64, secs % 86_400);

    // civil date from days since 1970-01-01 (Howard Hinnant's algorithm)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);

    format!("{year:04}{month:02}{day:02}-{:02}{:02}{:02}", rem / 3600, rem % 3600 / 60, rem % 60)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::config::writer::Value;

    #[test]
    fn timestamps() {
        assert_eq!(timestamp(UNIX_EPOCH), "19700101-000000");
        assert_eq!(timestamp(UNIX_EPOCH + Duration::from_secs(1_791_814_245)), "20261012-141045");
        assert_eq!(timestamp(UNIX_EPOCH + Duration::from_secs(951_782_400)), "20000229-000000");
    }

    #[test]
    fn saves_atomically_backs_up_once_and_recognises_its_own_write() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yaml");
        fs::write(&path, "# mine\ncom_port: COM4\n").unwrap();

        let mut store = ConfigStore::new(path.clone(), dir.path().join("backups"));
        let written = store.apply(&[Edit::set(&["com_port"], Value::Str("COM3".into()))]).unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), "# mine\ncom_port: COM3\n");
        assert!(store.is_own_write(&written));
        assert!(!store.is_own_write("# mine\ncom_port: COM5\n"));
        assert!(!dir.path().join("config.yaml.tmp").exists());

        let backups: Vec<_> = fs::read_dir(dir.path().join("backups")).unwrap().collect();
        assert_eq!(backups.len(), 1);
        let backup = backups.into_iter().next().unwrap().unwrap().path();
        assert_eq!(fs::read_to_string(backup).unwrap(), "# mine\ncom_port: COM4\n");

        store.apply(&[Edit::set(&["com_port"], Value::Str("COM5".into()))]).unwrap();
        assert_eq!(fs::read_dir(dir.path().join("backups")).unwrap().count(), 1, "one backup per run");
    }

    #[test]
    fn keeps_only_the_newest_backups() {
        let dir = tempfile::tempdir().unwrap();
        let backups = dir.path().join("backups");
        fs::create_dir_all(&backups).unwrap();
        for i in 0..7 {
            fs::write(backups.join(format!("config-2026010{i}-000000.yaml")), "").unwrap();
        }

        let path = dir.path().join("config.yaml");
        fs::write(&path, "x: 1\n").unwrap();
        ConfigStore::new(path, backups.clone()).apply(&[Edit::set(&["x"], Value::Int(2))]).unwrap();

        let mut names: Vec<String> =
            fs::read_dir(&backups).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().to_string()).collect();
        names.sort();
        assert_eq!(names.len(), BACKUPS_KEPT);
        assert_eq!(names[0], "config-20260103-000000.yaml");
    }

    #[test]
    fn a_refused_edit_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yaml");
        fs::write(&path, "slider_mapping: master\n").unwrap();

        let mut store = ConfigStore::new(path.clone(), dir.path().join("backups"));
        assert!(store.apply(&[Edit::set(&["slider_mapping", "0"], Value::Str("x".into()))]).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "slider_mapping: master\n");
        assert!(!dir.path().join("backups").exists());
    }
}
