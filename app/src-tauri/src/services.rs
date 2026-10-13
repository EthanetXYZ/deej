//! Starting and stopping deej's background work: the audio engine, the board connection, the config file
//! watcher and the coordinator that ties them together. None of it depends on a window being open.

use std::path::Path;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use deej_audio::AudioEngine;
use deej_core::config::Config;
use deej_core::config::store::ConfigStore;
use deej_core::location::{CONFIG_FILE_NAME, ConfigLocation};
use deej_platform::foreground::ForegroundApps;
use deej_serial::{DiagnosticsReader, SerialConnection};
use notify::{EventKind, RecursiveMode, Watcher};

use crate::coordinator::{Coordinator, Msg, Parts, Shared, UiEvent, reading_options, serial_settings};

pub struct Services {
    tx: Sender<Msg>,
    serial_diagnostics: DiagnosticsReader,
    coordinator: Option<JoinHandle<()>>,
    _watcher: Option<notify::RecommendedWatcher>,
}

impl Services {
    pub fn start(
        location: &ConfigLocation,
        config: Config,
        shared: Arc<Mutex<Shared>>,
        ui: Box<dyn Fn(UiEvent) + Send>,
    ) -> std::io::Result<Services> {
        let (tx, rx) = mpsc::channel();

        let audio_tx = tx.clone();
        let audio = AudioEngine::start(Box::new(move |event| {
            let _ = audio_tx.send(Msg::AudioEvent(event));
        }))?;

        let serial_tx = tx.clone();
        let serial = SerialConnection::start(
            serial_settings(&config),
            reading_options(&config),
            Box::new(move |event| {
                let _ = serial_tx.send(Msg::Serial(event));
            }),
        )?;

        let serial_diagnostics = serial.diagnostics_reader();
        let watcher = watch_config(&location.dir, tx.clone());

        let foreground = ForegroundApps::default();
        let coordinator = Coordinator::new(Parts {
            config,
            store: ConfigStore::new(location.config_path(), location.log_dir().join("config-backups")),
            preferences: location.preferences_path(),
            audio: Box::new(audio),
            hardware: Box::new(serial),
            foreground: Box::new(move || foreground.process_names()),
            ui,
            shared,
        });

        let thread = std::thread::Builder::new().name("deej-coordinator".into()).spawn(move || coordinator.run(rx))?;

        Ok(Services { tx, serial_diagnostics, coordinator: Some(thread), _watcher: watcher })
    }

    pub fn send(&self, msg: Msg) {
        let _ = self.tx.send(msg);
    }

    pub fn serial_diagnostics(&self) -> deej_serial::Diagnostics {
        self.serial_diagnostics.read()
    }

    /// Saves pending edits, then stops the coordinator, which stops the board connection and the audio engine.
    pub fn stop(&mut self) {
        if let Some(thread) = self.coordinator.take() {
            let _ = self.tx.send(Msg::Stop);
            let _ = thread.join();
            tracing::info!("Background services stopped");
        }
    }
}

impl Drop for Services {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Watches the config folder (editors often replace the file rather than writing to it).
fn watch_config(dir: &Path, tx: Sender<Msg>) -> Option<notify::RecommendedWatcher> {
    let handler = move |result: notify::Result<notify::Event>| {
        let Ok(event) = result else { return };
        let relevant = matches!(event.kind, EventKind::Create(_) | EventKind::Modify(_))
            && event.paths.iter().any(|p| p.file_name().is_some_and(|n| n.eq_ignore_ascii_case(CONFIG_FILE_NAME)));
        if relevant {
            let _ = tx.send(Msg::ConfigFileChanged);
        }
    };

    let mut watcher = match notify::recommended_watcher(handler) {
        Ok(watcher) => watcher,
        Err(err) => {
            tracing::warn!(error = %err, "Can't watch config.yaml for changes; edits need a restart");
            return None;
        }
    };

    if let Err(err) = watcher.watch(dir, RecursiveMode::NonRecursive) {
        tracing::warn!(error = %err, dir = %dir.display(), "Can't watch the config folder; edits need a restart");
        return None;
    }

    Some(watcher)
}
