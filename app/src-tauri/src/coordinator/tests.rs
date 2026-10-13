//! The coordinator with a fake audio backend and a fake board.

use std::sync::{Arc, Mutex};

use deej_audio::fake::FakeAudio;
use deej_core::audio::{EndpointRef, Flow, SessionInfo, SessionState, sample_snapshot, session_key};
use deej_core::config::writer::Value;
use deej_core::protocol::SliderMove;
use deej_serial::ConnectionState;

use super::*;

#[derive(Default)]
struct FakeHardware {
    calls: Arc<Mutex<Vec<String>>>,
}

impl Hardware for FakeHardware {
    fn set_settings(&self, s: SerialSettings) {
        self.calls.lock().unwrap().push(format!("settings {} {} dtr={} rts={}", s.port, s.baud_rate, s.dtr, s.rts));
    }
    fn set_options(&self, o: ReadingOptions) {
        self.calls.lock().unwrap().push(format!("options invert={} noise={}", o.invert, o.noise.as_str()));
    }
    fn resend_all(&self) {
        self.calls.lock().unwrap().push("resend_all".into());
    }
    fn reconnect(&self) {
        self.calls.lock().unwrap().push("reconnect".into());
    }
}

struct Harness {
    coordinator: Coordinator,
    audio: Arc<FakeAudio>,
    hardware: Arc<Mutex<Vec<String>>>,
    ui: Arc<Mutex<Vec<UiEvent>>>,
    shared: Arc<Mutex<Shared>>,
    dir: tempfile::TempDir,
}

/// Lets a shared FakeAudio be handed to the coordinator while the test keeps inspecting it.
struct SharedAudio(Arc<FakeAudio>);

impl AudioControl for SharedAudio {
    fn send(&self, command: Command) {
        self.0.send(command)
    }
}

const CONFIG: &str = "# my sliders
slider_mapping:
  0: master
  1: chrome.exe
  2: deej.unmapped
  3: deej.current
slider_sensitivity:
  default:
    curve: 2
com_port: COM3
";

fn harness(config_text: &str) -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.yaml");
    std::fs::write(&path, config_text).unwrap();

    let config = config::parse(config_text, None).unwrap().config;
    let audio = Arc::new(FakeAudio::new(sample_snapshot(), Box::new(|_| {})));
    let hardware = FakeHardware::default();
    let calls = hardware.calls.clone();
    let ui = Arc::new(Mutex::new(Vec::new()));
    let ui_sink = ui.clone();
    let shared = Arc::new(Mutex::new(Shared::default()));

    let mut coordinator = Coordinator::new(Parts {
        config,
        store: ConfigStore::new(path, dir.path().join("logs/config-backups")),
        preferences: dir.path().join("logs/preferences.yaml"),
        audio: Box::new(SharedAudio(audio.clone())),
        hardware: Box::new(hardware),
        foreground: Box::new(|| vec!["Spotify.exe".to_string()]),
        ui: Box::new(move |e| ui_sink.lock().unwrap().push(e)),
        shared: shared.clone(),
    });
    coordinator.handle(Msg::AudioEvent(AudioEvent::Snapshot(sample_snapshot())));

    Harness { coordinator, audio, hardware: calls, ui, shared, dir }
}

fn moves(positions: &[f32], moved: &[usize]) -> Msg {
    Msg::Serial(SerialEvent::Moves {
        moves: moved.iter().map(|&slider| SliderMove { slider, position: positions[slider] }).collect(),
        positions: positions.to_vec(),
        received: std::time::Instant::now(),
    })
}

fn last_volumes(audio: &FakeAudio) -> Vec<(VolumeTarget, f32)> {
    match audio.commands().last() {
        Some(Command::SetVolumes(sets)) => sets.clone(),
        other => panic!("expected SetVolumes, got {other:?}"),
    }
}

#[test]
fn slider_moves_set_their_targets_through_the_sensitivity_curve() {
    let mut h = harness(CONFIG);

    h.coordinator.handle(moves(&[0.5, 0.0, 0.0, 0.0], &[0]));
    assert_eq!(last_volumes(&h.audio), vec![(VolumeTarget::Endpoint(EndpointRef::Default(Flow::Render)), 0.25)]);

    h.coordinator.handle(moves(&[0.5, 1.0, 0.0, 0.0], &[1]));
    assert_eq!(last_volumes(&h.audio), vec![(VolumeTarget::Session(2), 1.0), (VolumeTarget::Session(3), 1.0)]);

    // deej.unmapped: discord and spotify (chrome is mapped, system never counts)
    h.coordinator.handle(moves(&[0.5, 1.0, 0.8, 0.0], &[2]));
    let unmapped: Vec<_> = last_volumes(&h.audio).into_iter().map(|(t, _)| t).collect();
    assert_eq!(unmapped, vec![VolumeTarget::Session(4), VolumeTarget::Session(5)]);

    // deej.current: the foreground app
    h.coordinator.handle(moves(&[0.5, 1.0, 0.8, 0.1], &[3]));
    assert_eq!(last_volumes(&h.audio), vec![(VolumeTarget::Session(5), 0.010000001)]);

    assert_eq!(h.shared.lock().unwrap().positions, vec![0.5, 1.0, 0.8, 0.1]);
}

#[test]
fn unmapped_sliders_send_nothing() {
    let mut h = harness(CONFIG);
    h.coordinator.handle(moves(&[0.0, 0.0, 0.0, 0.0, 0.7], &[4]));
    assert!(h.audio.commands().iter().all(|c| !matches!(c, Command::SetVolumes(_))));
}

fn new_chrome_session(id: u64) -> SessionInfo {
    SessionInfo {
        id,
        endpoint_id: "speakers".into(),
        pid: 999,
        process_name: "chrome.exe".into(),
        key: session_key("chrome.exe", false),
        display_name: String::new(),
        exe_path: None,
        is_system: false,
        state: SessionState::Active,
        volume: 1.0,
        muted: false,
    }
}

#[test]
fn an_app_that_starts_later_gets_its_sliders_volume_and_others_are_left_alone() {
    let mut h = harness(CONFIG);
    h.coordinator.handle(moves(&[0.5, 0.5, 0.5, 0.5], &[0, 1, 2, 3]));
    let before = h.audio.commands().len();

    let mut snapshot = sample_snapshot();
    snapshot.sessions.push(new_chrome_session(42));
    h.coordinator.handle(Msg::AudioEvent(AudioEvent::Snapshot(snapshot)));

    let commands = h.audio.commands();
    assert_eq!(commands.len(), before + 1);
    assert_eq!(last_volumes(&h.audio), vec![(VolumeTarget::Session(42), 0.25)]);
}

#[test]
fn nothing_is_applied_to_new_apps_before_a_slider_was_read() {
    let mut h = harness(CONFIG);
    let mut snapshot = sample_snapshot();
    snapshot.sessions.push(new_chrome_session(42));
    h.coordinator.handle(Msg::AudioEvent(AudioEvent::Snapshot(snapshot)));
    assert!(h.audio.commands().iter().all(|c| !matches!(c, Command::SetVolumes(_))));
}

#[test]
fn a_new_default_device_gets_the_master_sliders_volume() {
    let mut h = harness(CONFIG);
    h.coordinator.handle(moves(&[0.5, 0.0, 0.0, 0.0], &[0]));

    let mut snapshot = sample_snapshot();
    snapshot.endpoints[0].is_default = false;
    snapshot.endpoints[1].is_default = true;
    h.coordinator.handle(Msg::AudioEvent(AudioEvent::Snapshot(snapshot)));

    assert_eq!(last_volumes(&h.audio), vec![(VolumeTarget::Endpoint(EndpointRef::Default(Flow::Render)), 0.25)]);
}

#[test]
fn external_volume_changes_update_the_state_and_reach_the_ui() {
    let mut h = harness(CONFIG);
    h.coordinator.handle(Msg::AudioEvent(AudioEvent::VolumeChanged {
        target: ChangedTarget::Session(5),
        volume: 0.3,
        muted: true,
        external: true,
    }));

    let s = h.coordinator.snapshot().session(5).unwrap();
    assert_eq!((s.volume, s.muted), (0.3, true));
    assert!(matches!(h.ui.lock().unwrap().last(), Some(UiEvent::Volume { external: true, .. })));
}

#[test]
fn editing_the_file_reloads_it_and_reapplies_every_slider() {
    let mut h = harness(CONFIG);
    let path = h.dir.path().join("config.yaml");
    std::fs::write(&path, CONFIG.replace("COM3", "COM7").replace("curve: 2", "curve: 1") + "invert_sliders: true\n")
        .unwrap();

    h.coordinator.handle(Msg::ConfigFileChanged);
    h.coordinator.run_due(Instant::now() + RELOAD_DELAY);

    assert_eq!(h.coordinator.config.com_port, "COM7");
    assert_eq!(
        *h.hardware.lock().unwrap(),
        vec!["settings COM7 9600 dtr=true rts=false", "options invert=true noise=default", "resend_all"]
    );
    assert!(matches!(h.ui.lock().unwrap().last(), Some(UiEvent::ConfigReloaded { .. })));
}

#[test]
fn the_ui_learns_what_each_slider_controls() {
    let h = harness(CONFIG);
    let targets = h.shared.lock().unwrap().slider_targets.clone();

    assert_eq!(targets.len(), 4);
    assert_eq!(targets[0].endpoints, vec!["speakers"], "master is the current default device");
    assert_eq!(targets[1].sessions, vec![2, 3], "both chrome sessions");
    assert_eq!(targets[2].sessions, vec![4, 5], "unmapped: discord and spotify");
    assert!(targets[3].follows_focus && targets[3].sessions.is_empty());
}

#[test]
fn a_broken_file_keeps_the_previous_settings() {
    let mut h = harness(CONFIG);
    std::fs::write(h.dir.path().join("config.yaml"), "slider_mapping: [\n").unwrap();

    h.coordinator.handle(Msg::ConfigFileChanged);
    h.coordinator.run_due(Instant::now() + RELOAD_DELAY);

    assert_eq!(h.coordinator.config.com_port, "COM3");
    assert!(h.hardware.lock().unwrap().is_empty());
    assert!(h.shared.lock().unwrap().config_error.is_some());
    assert!(matches!(h.ui.lock().unwrap().last(), Some(UiEvent::ConfigError { .. })));
}

#[test]
fn edits_apply_at_once_and_are_saved_after_a_pause_without_reloading() {
    let mut h = harness(CONFIG);
    let path = h.dir.path().join("config.yaml");

    h.coordinator.handle(Msg::Edit(vec![Edit::set(&["slider_sensitivity", "default", "curve"], Value::Float(2.5))]));
    h.coordinator.handle(Msg::Edit(vec![Edit::set(&["slider_sensitivity", "default", "curve"], Value::Float(3.0))]));
    h.coordinator.handle(Msg::Edit(vec![Edit::set(&["slider_mapping", "4"], Value::Str("discord.exe".into()))]));

    // in effect immediately, not written yet
    assert_eq!(h.coordinator.config.slider_sensitivity.default.curve, Some(3.0));
    assert_eq!(h.coordinator.config.slider_mapping[&4], vec!["discord.exe"]);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), CONFIG);
    assert_eq!(h.coordinator.pending_edits(), 2, "edits to the same key are merged");

    h.coordinator.run_due(Instant::now() + SAVE_DELAY);
    let saved = std::fs::read_to_string(&path).unwrap();
    assert!(saved.starts_with("# my sliders\n"));
    assert!(saved.contains("    curve: 3\n") && saved.contains("  4: discord.exe\n"), "{saved}");
    assert!(matches!(h.ui.lock().unwrap().last(), Some(UiEvent::Saved)));

    // the watcher then reports our own write: no reload, no second round of re-applying
    h.hardware.lock().unwrap().clear();
    h.coordinator.handle(Msg::ConfigFileChanged);
    h.coordinator.run_due(Instant::now() + RELOAD_DELAY);
    assert!(h.hardware.lock().unwrap().is_empty());
}

#[test]
fn an_edit_that_cant_be_written_is_reported_and_not_applied() {
    let mut h = harness("slider_mapping: master\n");
    h.coordinator.handle(Msg::Edit(vec![Edit::set(&["slider_mapping", "1"], Value::Str("x.exe".into()))]));

    assert!(h.coordinator.config.slider_mapping.is_empty());
    assert!(matches!(h.ui.lock().unwrap().last(), Some(UiEvent::SaveFailed { .. })));
    assert_eq!(h.coordinator.pending_edits(), 0);
}

#[test]
fn hardware_status_reaches_the_ui() {
    let mut h = harness(CONFIG);
    let status = Status { port: "COM3".into(), baud_rate: 9600, state: ConnectionState::Connected };
    h.coordinator.handle(Msg::Serial(SerialEvent::Status(status.clone())));
    assert_eq!(h.shared.lock().unwrap().hardware, Some(status));
}

#[test]
fn waking_from_sleep_reconnects_and_rescans_after_a_delay() {
    let mut h = harness(CONFIG);
    h.coordinator.handle(Msg::Resumed);
    h.coordinator.run_due(Instant::now());
    assert!(!h.hardware.lock().unwrap().contains(&"reconnect".to_string()), "too early");

    h.coordinator.run_due(Instant::now() + RESUME_DELAY);
    assert!(h.hardware.lock().unwrap().contains(&"reconnect".to_string()));
    assert!(matches!(h.audio.commands().last(), Some(Command::Rescan)));

    // handled once
    let before = h.audio.commands().len();
    h.coordinator.run_due(Instant::now() + RESUME_DELAY * 2);
    assert_eq!(h.audio.commands().len(), before);
}

#[test]
fn switching_the_default_device_brings_it_to_the_master_slider() {
    let mut h = harness(CONFIG);
    h.coordinator.handle(moves(&[0.5, 0.0, 0.0, 0.0], &[0]));

    let other = sample_snapshot().endpoints.into_iter().find(|e| e.flow == Flow::Render && !e.is_default).unwrap();
    h.coordinator.handle(Msg::SetDefaultEndpoint(other.id.clone()));
    // the fake reports the new default as a snapshot, like Windows' notification does for the real engine
    let snapshot = h.audio.snapshot();
    assert_eq!(snapshot.default_endpoint(Flow::Render).unwrap().id, other.id);
    h.coordinator.handle(Msg::AudioEvent(AudioEvent::Snapshot(snapshot)));

    // "master" stays "whatever is default", so the engine applies it to the new device
    assert_eq!(last_volumes(&h.audio), vec![(VolumeTarget::Endpoint(EndpointRef::Default(Flow::Render)), 0.25)]);
}

#[test]
fn mute_requests_go_to_the_audio_engine() {
    let mut h = harness(CONFIG);
    let master = VolumeTarget::Endpoint(EndpointRef::Default(Flow::Render));
    h.coordinator.handle(Msg::SetMute { target: master.clone(), muted: true });
    assert_eq!(h.audio.commands().last(), Some(&Command::SetMute { target: master, muted: true }));
}

/// The UI (app/src/ipc.ts) reads these exact field names; a rename on either side must fail here.
#[test]
fn json_shapes_match_what_the_ui_expects() {
    let mut h = harness(CONFIG);
    h.coordinator.handle(moves(&[0.5, 0.0, 0.0, 0.0], &[0]));
    h.coordinator.handle(Msg::Serial(SerialEvent::Status(Status {
        port: "COM3".into(),
        baud_rate: 9600,
        state: ConnectionState::Failed { message: "boom".into() },
    })));

    let json = serde_json::to_value(h.shared.lock().unwrap().clone()).unwrap();
    let keys = |v: &serde_json::Value| v.as_object().unwrap().keys().cloned().collect::<Vec<_>>();

    assert_eq!(
        keys(&json),
        ["audio", "config", "configError", "diagnostics", "hardware", "positions", "sliderTargets"]
    );
    assert_eq!(
        keys(&json["config"]),
        [
            "baud_rate",
            "com_port",
            "invert_sliders",
            "next",
            "noise_reduction",
            "slider_mapping",
            "slider_sensitivity",
            "unmapped_exclude"
        ]
    );
    assert_eq!(json["config"]["slider_mapping"]["1"], serde_json::json!(["chrome.exe"]));
    assert_eq!(keys(&json["config"]["slider_sensitivity"]), ["default", "sliders"]);
    assert_eq!(keys(&json["config"]["next"]["serial"]), ["dtr", "reconnect_interval_ms", "rts"]);
    assert_eq!(
        json["hardware"],
        serde_json::json!({"port": "COM3", "baudRate": 9600, "state": "failed", "message": "boom"})
    );
    assert_eq!(keys(&json["sliderTargets"][0]), ["endpoints", "followsFocus", "sessions", "slider"]);
    assert_eq!(keys(&json["audio"]), ["available", "endpoints", "sessions"]);
    assert_eq!(
        keys(&json["audio"]["sessions"][0]),
        [
            "displayName",
            "endpointId",
            "exePath",
            "id",
            "isSystem",
            "key",
            "muted",
            "pid",
            "processName",
            "state",
            "volume"
        ]
    );
    assert_eq!(
        keys(&json["audio"]["endpoints"][0]),
        ["description", "flow", "id", "isDefault", "muted", "name", "volume"]
    );

    let event = serde_json::to_value(UiEvent::Volume {
        target: ChangedTarget::Session(5),
        volume: 0.5,
        muted: false,
        external: true,
    })
    .unwrap();
    assert_eq!(
        event,
        serde_json::json!({"type": "volume", "target": {"kind": "session", "id": 5}, "volume": 0.5, "muted": false, "external": true})
    );
    assert_eq!(serde_json::to_value(UiEvent::ConfigChanged).unwrap(), serde_json::json!({"type": "configChanged"}));
    assert_eq!(
        serde_json::to_value(UiEvent::SaveFailed { message: "x".into() }).unwrap(),
        serde_json::json!({"type": "saveFailed", "message": "x"})
    );

    let connecting = Status { port: "COM3".into(), baud_rate: 9600, state: ConnectionState::NotFound };
    assert_eq!(serde_json::to_value(connecting).unwrap()["state"], "notFound");
}
