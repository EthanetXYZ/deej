//! Audio types shared by the engine (`deej-audio`), the fake backend and the mapping logic. Platform-independent.
//!
//! The engine never hands out COM objects: the rest of deej sees endpoints and sessions only through these plain
//! values, identified by the endpoint's Windows device id and a [`SessionId`] the engine assigns.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Stable id the engine gives an audio session for as long as it exists (based on the session's instance
/// identifier, so re-scans keep the same id).
pub type SessionId = u64;

/// The `system` target: the "System sounds" session(s), the same key the Go deej used.
pub const SYSTEM_SESSION_KEY: &str = "system";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Flow {
    /// Playback device.
    Render,
    /// Recording device.
    Capture,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionState {
    /// Currently playing.
    Active,
    /// Opened but not playing. deej still controls these, like the Go version did.
    Inactive,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EndpointInfo {
    /// Windows endpoint id, e.g. `{0.0.0.00000000}.{guid}`.
    pub id: String,
    /// Friendly name, e.g. "Speakers (Realtek(R) Audio)". This is what device targets in `config.yaml` match.
    pub name: String,
    /// Short description, e.g. "Speakers".
    pub description: String,
    pub flow: Flow,
    /// Whether this is the default device for its flow (console role, like the Go deej's `master`/`mic`).
    pub is_default: bool,
    pub volume: f32,
    pub muted: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub id: SessionId,
    /// The playback device this session plays on.
    pub endpoint_id: String,
    pub pid: u32,
    /// Executable file name as Windows reports it, e.g. "Spotify.exe". Empty for system sounds.
    pub process_name: String,
    /// What `config.yaml` targets match against: the lowercased exe name, or `system`.
    pub key: String,
    /// The session's own display name, if the app set one. Often empty.
    pub display_name: String,
    /// Full path of the executable, when Windows lets us read it (not for some elevated processes).
    pub exe_path: Option<String>,
    pub is_system: bool,
    pub state: SessionState,
    pub volume: f32,
    pub muted: bool,
}

/// Everything the engine currently knows.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioSnapshot {
    /// False while Windows audio can't be used at all (no audio service, or no devices, e.g. over Remote Desktop).
    pub available: bool,
    pub endpoints: Vec<EndpointInfo>,
    pub sessions: Vec<SessionInfo>,
}

impl AudioSnapshot {
    pub fn default_endpoint(&self, flow: Flow) -> Option<&EndpointInfo> {
        self.endpoints.iter().find(|e| e.flow == flow && e.is_default)
    }

    pub fn endpoint(&self, id: &str) -> Option<&EndpointInfo> {
        self.endpoints.iter().find(|e| e.id == id)
    }

    pub fn session(&self, id: SessionId) -> Option<&SessionInfo> {
        self.sessions.iter().find(|s| s.id == id)
    }

    /// Sessions whose key matches, e.g. every `chrome.exe` session on every device.
    pub fn sessions_with_key<'a>(&'a self, key: &'a str) -> impl Iterator<Item = &'a SessionInfo> + 'a {
        self.sessions.iter().filter(move |s| s.key == key)
    }
}

/// The key a session is matched by in `config.yaml`: `system` for system sounds, otherwise the lowercased
/// executable name (the Go deej's `Key()`).
pub fn session_key(process_name: &str, is_system: bool) -> String {
    if is_system { SYSTEM_SESSION_KEY.to_string() } else { process_name.to_lowercase() }
}

/// Which endpoint a command is for. The default devices are resolved by the engine at the moment the command
/// runs, so a command sent just as the default device changes still goes to the right place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EndpointRef {
    Id(String),
    Default(Flow),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VolumeTarget {
    Endpoint(EndpointRef),
    Session(SessionId),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// Set one volume (0.0–1.0). Skipped if the volume is already exactly that, like the Go deej.
    SetVolume {
        target: VolumeTarget,
        volume: f32,
    },
    /// Set several volumes at once (one slider move can hit many sessions).
    SetVolumes(Vec<(VolumeTarget, f32)>),
    SetMute {
        target: VolumeTarget,
        muted: bool,
    },
    /// Make a device the default for its direction (playback or recording), for every role.
    SetDefaultEndpoint {
        id: String,
    },
    /// Re-enumerate every device and session now.
    Rescan,
    /// Send a [`AudioEvent::Snapshot`] with the current state.
    RequestSnapshot,
    /// How often the engine double-checks its state against Windows (notifications are the primary source).
    SetReconcileInterval(Duration),
}

/// What changed volume.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind", content = "id")]
pub enum ChangedTarget {
    Endpoint(String),
    Session(SessionId),
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum AudioEvent {
    /// The set of devices or sessions changed (or a snapshot was requested).
    Snapshot(AudioSnapshot),
    /// A volume or mute state changed. `external` is false when deej itself made the change.
    #[serde(rename_all = "camelCase")]
    VolumeChanged { target: ChangedTarget, volume: f32, muted: bool, external: bool },
}

/// Something that accepts audio commands: the real engine, or [`crate::fake::FakeAudio`] in tests.
pub trait AudioControl: Send + Sync {
    fn send(&self, command: Command);
}

/// Receives engine events. Called from the engine's thread, so it should hand events off quickly.
pub type EventSink = Box<dyn Fn(AudioEvent) + Send + Sync + 'static>;

/// Volumes outside 0–1 (or NaN) are clamped before reaching Windows, which would reject them.
pub fn clamp_volume(volume: f32) -> f32 {
    if volume.is_nan() { 0.0 } else { volume.clamp(0.0, 1.0) }
}

#[doc(hidden)]
/// A small snapshot for tests: speakers (default) + headset playback, a mic, and a few apps.
pub fn sample_snapshot() -> AudioSnapshot {
    let endpoint = |id: &str, name: &str, flow, is_default| EndpointInfo {
        id: id.into(),
        name: name.into(),
        description: name.split(" (").next().unwrap_or(name).into(),
        flow,
        is_default,
        volume: 0.5,
        muted: false,
    };

    let session = |id, endpoint_id: &str, pid, process: &str| SessionInfo {
        id,
        endpoint_id: endpoint_id.into(),
        pid,
        process_name: process.into(),
        key: session_key(process, pid == 0),
        display_name: String::new(),
        exe_path: None,
        is_system: pid == 0,
        state: SessionState::Active,
        volume: 1.0,
        muted: false,
    };

    AudioSnapshot {
        available: true,
        endpoints: vec![
            endpoint("speakers", "Speakers (Realtek(R) Audio)", Flow::Render, true),
            endpoint("headset", "Headset Earphone (CORSAIR VIRTUOSO SE Wireless Gaming Headset)", Flow::Render, false),
            endpoint("mic", "Headset Microphone (CORSAIR VIRTUOSO SE Wireless Gaming Headset)", Flow::Capture, true),
        ],
        sessions: vec![
            session(1, "speakers", 0, ""),
            session(2, "speakers", 100, "chrome.exe"),
            session(3, "speakers", 101, "chrome.exe"),
            session(4, "headset", 102, "Discord.exe"),
            session(5, "speakers", 103, "Spotify.exe"),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_keys_match_the_go_deej() {
        assert_eq!(session_key("Spotify.exe", false), "spotify.exe");
        assert_eq!(session_key("", true), "system");
    }

    #[test]
    fn volumes_are_clamped() {
        assert_eq!(clamp_volume(1.2), 1.0);
        assert_eq!(clamp_volume(-0.1), 0.0);
        assert_eq!(clamp_volume(f32::NAN), 0.0);
        assert_eq!(clamp_volume(0.5), 0.5);
    }
}
