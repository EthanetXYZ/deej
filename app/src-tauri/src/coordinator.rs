//! The coordinator: the one place that owns deej's state and connects the board, the audio engine and the config.
//!
//! Everything arrives as a [`Msg`] on one queue (slider moves, audio changes, config file changes, edits from the
//! UI) and is handled in order on the coordinator's thread, so no state is shared between threads except the
//! [`Shared`] view that the UI reads (ARCHITECTURE.md §3.5).

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use deej_core::audio::{AudioControl, AudioEvent, AudioSnapshot, ChangedTarget, Command, EndpointRef, VolumeTarget};
use deej_core::config::store::ConfigStore;
use deej_core::config::writer::{Edit, apply_edits};
use deej_core::config::{self, Config, Diagnostic};
use deej_core::mapping::{CURRENT, Resolver, new_targets};
use deej_serial::{ReadingOptions, SerialEvent, SerialSettings, Status};
use serde::Serialize;

/// How long after the last edit the config file is written (the fork used the same delay).
pub const SAVE_DELAY: Duration = Duration::from_millis(400);

/// How long to wait after a file change notification before reloading (editors often write in several steps).
pub const RELOAD_DELAY: Duration = Duration::from_millis(250);

/// How long after waking from sleep the board is reconnected and audio re-scanned (USB and audio devices need a
/// moment to come back).
pub const RESUME_DELAY: Duration = Duration::from_secs(3);

pub enum Msg {
    Serial(SerialEvent),
    AudioEvent(AudioEvent),
    /// The config file (or its folder) changed on disk.
    ConfigFileChanged,
    /// Settings changed in the UI: applied right away, saved to config.yaml shortly after.
    Edit(Vec<Edit>),
    /// Re-enumerate audio and re-apply every slider ("Re-scan").
    Rescan,
    /// Close and reopen the serial port ("Restart hardware connection").
    ReconnectHardware,
    /// Mute or unmute a device or app (the tray's "Mute" item).
    SetMute {
        target: VolumeTarget,
        muted: bool,
    },
    /// Make a device the default playback or recording device (the tray's device list).
    SetDefaultEndpoint(String),
    /// The PC woke from sleep: reconnect the board and re-scan audio shortly.
    Resumed,
    Stop,
}

/// What the coordinator needs from the board connection (a fake in tests).
pub trait Hardware: Send {
    fn set_settings(&self, settings: SerialSettings);
    fn set_options(&self, options: ReadingOptions);
    fn resend_all(&self);
    fn reconnect(&self);
}

impl Hardware for deej_serial::SerialConnection {
    fn set_settings(&self, settings: SerialSettings) {
        deej_serial::SerialConnection::set_settings(self, settings)
    }
    fn set_options(&self, options: ReadingOptions) {
        deej_serial::SerialConnection::set_options(self, options)
    }
    fn resend_all(&self) {
        deej_serial::SerialConnection::resend_all(self)
    }
    fn reconnect(&self) {
        deej_serial::SerialConnection::reconnect(self)
    }
}

/// Events for the UI.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum UiEvent {
    Sliders {
        positions: Vec<f32>,
    },
    Hardware {
        status: Status,
    },
    Audio {
        snapshot: AudioSnapshot,
    },
    #[serde(rename_all = "camelCase")]
    Volume {
        target: ChangedTarget,
        volume: f32,
        muted: bool,
        external: bool,
    },
    ConfigReloaded {
        diagnostics: Vec<Diagnostic>,
    },
    /// The settings in effect changed (from the file or the UI); the UI re-reads its state.
    ConfigChanged,
    ConfigError {
        message: String,
    },
    Saved,
    SaveFailed {
        message: String,
    },
}

/// The state the UI reads (through Tauri commands), kept up to date by the coordinator.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Shared {
    pub config: Config,
    pub config_error: Option<String>,
    pub diagnostics: Vec<Diagnostic>,
    pub audio: AudioSnapshot,
    pub positions: Vec<f32>,
    pub hardware: Option<Status>,
    /// What each mapped slider controls right now (for the UI's "Slider 2" badges).
    pub slider_targets: Vec<SliderTargets>,
}

/// The sessions and devices a slider currently controls. `deej.current` depends on the focused window, so it's
/// reported as a flag rather than resolved.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SliderTargets {
    pub slider: usize,
    pub sessions: Vec<u64>,
    pub endpoints: Vec<String>,
    pub follows_focus: bool,
}

/// Resolves every mapped slider against the current audio state.
pub fn resolve_slider_targets(config: &Config, snapshot: &AudioSnapshot) -> Vec<SliderTargets> {
    let resolver = Resolver::new(config, snapshot);
    config
        .slider_mapping
        .iter()
        .map(|(&slider, targets)| {
            let mut entry = SliderTargets {
                slider,
                follows_focus: targets.iter().any(|t| t.eq_ignore_ascii_case(CURRENT)),
                ..Default::default()
            };
            for target in resolver.resolve(targets, &mut Vec::new) {
                match target {
                    VolumeTarget::Session(id) => entry.sessions.push(id),
                    VolumeTarget::Endpoint(EndpointRef::Id(id)) => entry.endpoints.push(id),
                    VolumeTarget::Endpoint(EndpointRef::Default(flow)) => {
                        if let Some(e) = snapshot.default_endpoint(flow) {
                            entry.endpoints.push(e.id.clone());
                        }
                    }
                }
            }
            entry
        })
        .collect()
}

pub fn serial_settings(config: &Config) -> SerialSettings {
    SerialSettings {
        port: config.com_port.clone(),
        baud_rate: config.baud_rate,
        dtr: config.next.serial.dtr,
        rts: config.next.serial.rts,
        reconnect_interval: Duration::from_millis(u64::from(config.next.serial.reconnect_interval_ms)),
    }
}

pub fn reading_options(config: &Config) -> ReadingOptions {
    ReadingOptions { invert: config.invert_sliders, noise: config.noise_reduction }
}

pub struct Coordinator {
    pub config: Config,
    store: ConfigStore,
    preferences: PathBuf,

    audio: Box<dyn AudioControl>,
    hardware: Box<dyn Hardware>,
    foreground: Box<dyn Fn() -> Vec<String> + Send>,
    ui: Box<dyn Fn(UiEvent) + Send>,
    shared: Arc<Mutex<Shared>>,

    snapshot: AudioSnapshot,
    positions: Vec<f32>,

    /// Edits not yet written, merged by path.
    pending: Vec<Edit>,
    save_at: Option<Instant>,
    reload_at: Option<Instant>,
    resume_at: Option<Instant>,
}

pub struct Parts {
    pub config: Config,
    pub store: ConfigStore,
    pub preferences: PathBuf,
    pub audio: Box<dyn AudioControl>,
    pub hardware: Box<dyn Hardware>,
    pub foreground: Box<dyn Fn() -> Vec<String> + Send>,
    pub ui: Box<dyn Fn(UiEvent) + Send>,
    pub shared: Arc<Mutex<Shared>>,
}

impl Coordinator {
    pub fn new(parts: Parts) -> Coordinator {
        let coordinator = Coordinator {
            config: parts.config,
            store: parts.store,
            preferences: parts.preferences,
            audio: parts.audio,
            hardware: parts.hardware,
            foreground: parts.foreground,
            ui: parts.ui,
            shared: parts.shared,
            snapshot: AudioSnapshot::default(),
            positions: Vec::new(),
            pending: Vec::new(),
            save_at: None,
            reload_at: None,
            resume_at: None,
        };
        coordinator.shared.lock().unwrap().config = coordinator.config.clone();
        coordinator
    }

    /// Runs until [`Msg::Stop`] (or every sender is gone). Pending edits are saved before returning.
    pub fn run(mut self, rx: Receiver<Msg>) {
        loop {
            let deadline = [self.save_at, self.reload_at, self.resume_at].into_iter().flatten().min();
            let msg = match deadline {
                Some(at) => match rx.recv_timeout(at.saturating_duration_since(Instant::now())) {
                    Ok(msg) => Some(msg),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => break,
                },
                None => match rx.recv() {
                    Ok(msg) => Some(msg),
                    Err(_) => break,
                },
            };

            if let Some(msg) = msg {
                if matches!(msg, Msg::Stop) {
                    break;
                }
                self.handle(msg);
            }

            self.run_due(Instant::now());
        }

        self.save_now();
    }

    /// Runs the delayed work that's due: saving edits, reloading a changed file, recovering after sleep.
    pub fn run_due(&mut self, now: Instant) {
        if self.save_at.is_some_and(|at| now >= at) {
            self.save_now();
        }
        if self.reload_at.is_some_and(|at| now >= at) {
            self.reload_at = None;
            self.reload_from_file();
        }
        if self.resume_at.is_some_and(|at| now >= at) {
            self.resume_at = None;
            tracing::info!("Woke from sleep: reconnecting the board and re-scanning audio");
            self.hardware.reconnect();
            self.audio.send(Command::Rescan);
        }
    }

    pub fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Serial(SerialEvent::Moves { moves, positions, received }) => {
                self.positions = positions;
                let sets = {
                    let resolver = Resolver::new(&self.config, &self.snapshot);
                    let mut sets = Vec::new();
                    for m in &moves {
                        sets.extend(resolver.slider_volumes(&self.config, m.slider, m.position, &mut *self.foreground));
                    }
                    sets
                };
                if !sets.is_empty() {
                    let count = sets.len();
                    self.audio.send(Command::SetVolumes(sets));
                    // with the audio engine's "Command done" timing, this gives slider → volume latency
                    tracing::debug!(
                        ?moves,
                        count,
                        since_line_us = received.elapsed().as_micros() as u64,
                        "Slider moved"
                    );
                }
                self.shared.lock().unwrap().positions = self.positions.clone();
                (self.ui)(UiEvent::Sliders { positions: self.positions.clone() });
            }

            Msg::Serial(SerialEvent::Status(status)) => {
                self.shared.lock().unwrap().hardware = Some(status.clone());
                (self.ui)(UiEvent::Hardware { status });
            }

            Msg::AudioEvent(AudioEvent::Snapshot(snapshot)) => {
                let sets = self.volumes_for_new_targets(&snapshot);
                self.snapshot = snapshot;
                if !sets.is_empty() {
                    tracing::debug!(count = sets.len(), "Bringing new apps/devices to their sliders' volume");
                    self.audio.send(Command::SetVolumes(sets));
                }
                {
                    let mut shared = self.shared.lock().unwrap();
                    shared.audio = self.snapshot.clone();
                    shared.slider_targets = resolve_slider_targets(&self.config, &self.snapshot);
                }
                (self.ui)(UiEvent::Audio { snapshot: self.snapshot.clone() });
            }

            Msg::AudioEvent(AudioEvent::VolumeChanged { target, volume, muted, external }) => {
                let levels = match &target {
                    ChangedTarget::Session(id) => {
                        self.snapshot.sessions.iter_mut().find(|s| s.id == *id).map(|s| (&mut s.volume, &mut s.muted))
                    }
                    ChangedTarget::Endpoint(id) => {
                        self.snapshot.endpoints.iter_mut().find(|e| e.id == *id).map(|e| (&mut e.volume, &mut e.muted))
                    }
                };
                if let Some((v, m)) = levels {
                    *v = volume;
                    *m = muted;
                }
                self.shared.lock().unwrap().audio = self.snapshot.clone();
                (self.ui)(UiEvent::Volume { target, volume, muted, external });
            }

            Msg::ConfigFileChanged => self.reload_at = Some(Instant::now() + RELOAD_DELAY),

            Msg::Edit(edits) => self.edit(edits),

            Msg::Rescan => {
                self.audio.send(Command::Rescan);
                self.hardware.resend_all();
            }

            Msg::ReconnectHardware => self.hardware.reconnect(),

            Msg::SetMute { target, muted } => self.audio.send(Command::SetMute { target, muted }),

            Msg::SetDefaultEndpoint(id) => self.audio.send(Command::SetDefaultEndpoint { id }),

            Msg::Resumed => self.resume_at = Some(Instant::now() + RESUME_DELAY),

            Msg::Stop => {}
        }
    }

    /// Slider volumes for sessions and devices that just appeared (or a default device that just changed), for
    /// sliders whose position is known. Apps that were already there keep whatever volume they have.
    fn volumes_for_new_targets(&self, after: &AudioSnapshot) -> Vec<(VolumeTarget, f32)> {
        let resolver = Resolver::new(&self.config, after);
        let mut sets = Vec::new();
        let mut foreground = || (self.foreground)();

        for (&slider, targets) in &self.config.slider_mapping {
            let Some(&position) = self.positions.get(slider).filter(|p| **p >= 0.0) else { continue };
            let volume = self.config.slider_sensitivity.for_slider(slider).apply(position);

            for target in new_targets(&self.snapshot, after, resolver.resolve(targets, &mut foreground)) {
                sets.push((target, volume));
            }
        }

        sets
    }

    /// Switches to a new config: serial settings, how readings are read, and every slider re-applied.
    fn apply_config(&mut self, config: Config, diagnostics: Vec<Diagnostic>) {
        self.hardware.set_settings(serial_settings(&config));
        self.hardware.set_options(reading_options(&config));
        self.config = config;

        // like the Go deej: a config change re-applies every slider (new mappings take effect immediately)
        self.hardware.resend_all();

        {
            let mut shared = self.shared.lock().unwrap();
            shared.config = self.config.clone();
            shared.config_error = None;
            shared.diagnostics = diagnostics;
            shared.slider_targets = resolve_slider_targets(&self.config, &self.snapshot);
        }
        (self.ui)(UiEvent::ConfigChanged);
    }

    fn reload_from_file(&mut self) {
        let Ok(source) = self.store.read() else {
            tracing::warn!(path = %self.store.path().display(), "Config file changed but can't be read");
            return;
        };

        if self.store.is_own_write(&source) {
            return;
        }

        // the user edited the file: any unsaved edits from the UI would be based on the old contents
        if !self.pending.is_empty() {
            tracing::warn!(
                count = self.pending.len(),
                "Dropping unsaved setting changes; config.yaml was edited directly"
            );
            self.pending.clear();
            self.save_at = None;
        }

        let preferences = std::fs::read_to_string(&self.preferences).ok();
        match config::parse(&source, preferences.as_deref()) {
            Ok(loaded) => {
                tracing::info!(sliders = loaded.config.slider_mapping.len(), "Config reloaded");
                let diagnostics = loaded.diagnostics.clone();
                self.apply_config(loaded.config, loaded.diagnostics);
                (self.ui)(UiEvent::ConfigReloaded { diagnostics });
            }
            Err(err) => {
                // keep running with the previous settings, like the original
                tracing::warn!(error = %err, "Config reload failed, keeping the previous settings");
                self.shared.lock().unwrap().config_error = Some(err.to_string());
                (self.ui)(UiEvent::ConfigError { message: err.to_string() });
            }
        }
    }

    /// Applies edits in memory now (so the volume follows a dragged control), and saves them shortly after.
    fn edit(&mut self, edits: Vec<Edit>) {
        for edit in edits {
            // a newer edit replaces older ones for the same key or anything inside it
            self.pending.retain(|p| !(p.path.len() >= edit.path.len() && p.path[..edit.path.len()] == edit.path[..]));
            self.pending.push(edit);
        }

        let source = match self.store.read() {
            Ok(source) => source,
            Err(err) => return self.report_save_failure(err.to_string()),
        };

        let preferences = std::fs::read_to_string(&self.preferences).ok();
        match apply_edits(&source, &self.pending)
            .map_err(|e| e.to_string())
            .and_then(|text| config::parse(&text, preferences.as_deref()).map_err(|e| e.to_string()))
        {
            Ok(loaded) => {
                self.apply_config(loaded.config, loaded.diagnostics);
                self.save_at = Some(Instant::now() + SAVE_DELAY);
            }
            Err(message) => {
                self.pending.clear();
                self.report_save_failure(message);
            }
        }
    }

    fn save_now(&mut self) {
        self.save_at = None;
        if self.pending.is_empty() {
            return;
        }

        let edits = std::mem::take(&mut self.pending);
        match self.store.apply(&edits) {
            Ok(_) => {
                tracing::info!(edits = edits.len(), "Saved config.yaml");
                (self.ui)(UiEvent::Saved);
            }
            Err(err) => self.report_save_failure(err.to_string()),
        }
    }

    fn report_save_failure(&mut self, message: String) {
        tracing::warn!(%message, "Couldn't save a setting");
        (self.ui)(UiEvent::SaveFailed { message });
    }

    #[cfg(test)]
    pub fn pending_edits(&self) -> usize {
        self.pending.len()
    }

    #[cfg(test)]
    pub fn snapshot(&self) -> &AudioSnapshot {
        &self.snapshot
    }
}

#[cfg(test)]
mod tests;
