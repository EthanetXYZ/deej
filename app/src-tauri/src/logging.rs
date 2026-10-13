//! Logging setup, keeping the Go deej's file layout so existing troubleshooting advice still applies:
//!
//! - `logs/deej-latest-run.log`: this run (the previous run's file is kept as `deej-previous-run.log`)
//! - `logs/deej-crash.log`: written by the panic hook; a non-empty one from the last run is kept as
//!   `deej-crash-previous.log`
//!
//! Debug builds also log to stderr. `--verbose` (or `-v`) lowers the level to debug, and `DEEJ_LOG` accepts a
//! full `tracing` filter (e.g. `DEEJ_LOG=deej_next=trace`) for development.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer, fmt};

pub const LATEST_LOG: &str = "deej-latest-run.log";
pub const PREVIOUS_LOG: &str = "deej-previous-run.log";
pub const CRASH_LOG: &str = "deej-crash.log";
pub const PREVIOUS_CRASH_LOG: &str = "deej-crash-previous.log";

static CRASH_LOG_PATH: OnceLock<PathBuf> = OnceLock::new();

/// Keeps the background log writer alive; dropping it flushes and stops file logging.
pub struct LogGuard {
    _file: Option<WorkerGuard>,
}

/// Starts logging. Never fails: if the log folder can't be used, logging falls back to stderr only.
/// With `write_files` off (another deej is running and owns the log files), nothing is written to logs/.
pub fn init(log_dir: &Path, verbose: bool, write_files: bool) -> LogGuard {
    let default_level = if verbose { "debug" } else { "info" };
    let filter = || EnvFilter::try_from_env("DEEJ_LOG").unwrap_or_else(|_| EnvFilter::new(default_level));

    let opened = if write_files { Some(open_log_file(log_dir)) } else { None };
    let (file_layer, guard, file_error) = match opened {
        None => (None, None, None),
        Some(opened) => match opened {
            Ok(file) => {
                let (writer, guard) = tracing_appender::non_blocking(file);
                let layer = fmt::layer().with_writer(writer).with_ansi(false).with_target(true).with_filter(filter());
                (Some(layer), Some(guard), None)
            }
            Err(err) => (None, None, Some(err)),
        },
    };

    let stderr_layer = cfg!(debug_assertions)
        .then(|| fmt::layer().with_writer(std::io::stderr).with_target(true).with_filter(filter()));

    tracing_subscriber::registry().with(file_layer).with(stderr_layer).init();

    if let Some(err) = file_error {
        tracing::warn!(dir = %log_dir.display(), error = %err, "Couldn't open the log file, logging to stderr only");
    }

    if write_files {
        install_panic_hook(log_dir);
    }

    LogGuard { _file: guard }
}

/// Moves the last run's log aside and opens a fresh one.
fn open_log_file(log_dir: &Path) -> std::io::Result<File> {
    fs::create_dir_all(log_dir)?;

    let latest = log_dir.join(LATEST_LOG);
    let previous = log_dir.join(PREVIOUS_LOG);
    let _ = fs::remove_file(&previous);
    let _ = fs::rename(&latest, &previous);

    File::create(latest)
}

/// Writes panics (with a backtrace) to `deej-crash.log` before the default handler runs.
fn install_panic_hook(log_dir: &Path) {
    let crash_log = log_dir.join(CRASH_LOG);
    rotate_crash_log(log_dir);
    let _ = CRASH_LOG_PATH.set(crash_log);

    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let backtrace = std::backtrace::Backtrace::force_capture();
        let thread = std::thread::current();
        let report = format!(
            "deej crashed\nversion: {}\nthread: {}\n{info}\n\nbacktrace:\n{backtrace}\n",
            crate::version_string(),
            thread.name().unwrap_or("<unnamed>"),
        );

        if let Some(path) = CRASH_LOG_PATH.get() {
            if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
                let _ = file.write_all(report.as_bytes());
            }
        }

        tracing::error!(%info, "Panic");
        default_hook(info);
    }));
}

/// A non-empty crash log means the previous run crashed: keep it as `deej-crash-previous.log`.
fn rotate_crash_log(log_dir: &Path) {
    let crash_log = log_dir.join(CRASH_LOG);
    if fs::metadata(&crash_log).is_ok_and(|m| m.len() > 0) {
        let previous = log_dir.join(PREVIOUS_CRASH_LOG);
        let _ = fs::remove_file(&previous);
        let _ = fs::rename(&crash_log, previous);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_files_rotate_per_run() {
        let dir = tempfile::tempdir().unwrap();

        let mut first = open_log_file(dir.path()).unwrap();
        first.write_all(b"run 1").unwrap();
        drop(first);

        let _second = open_log_file(dir.path()).unwrap();
        assert_eq!(fs::read_to_string(dir.path().join(PREVIOUS_LOG)).unwrap(), "run 1");
        assert_eq!(fs::read_to_string(dir.path().join(LATEST_LOG)).unwrap(), "");
    }

    #[test]
    fn only_non_empty_crash_logs_are_kept() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(CRASH_LOG), "").unwrap();
        rotate_crash_log(dir.path());
        assert!(!dir.path().join(PREVIOUS_CRASH_LOG).exists());

        fs::write(dir.path().join(CRASH_LOG), "boom").unwrap();
        rotate_crash_log(dir.path());
        assert_eq!(fs::read_to_string(dir.path().join(PREVIOUS_CRASH_LOG)).unwrap(), "boom");
        assert!(!dir.path().join(CRASH_LOG).exists());
    }
}
