//! Integration tests against the real Windows audio stack. They need real audio devices, so they're ignored by
//! default (CI and Remote Desktop sessions have none, and sandboxed shells can't see them). Run them with:
//!
//! ```text
//! cargo test -p deej-audio --test windows_engine -- --ignored --test-threads=1
//! ```
//!
//! They try not to disturb the person at the machine: volume and mute changes go to a playback device that isn't
//! the default (unless there's only one), the default device is only nudged by 1%, every change is restored, and
//! the test "app" plays silence.
#![cfg(windows)]

use std::process::{Child, Command as Process};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use deej_audio::*;
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{IMMDeviceEnumerator, MMDeviceEnumerator};
use windows::Win32::System::Com::{CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize};
use windows_core::HSTRING;

/// The tests change shared machine state, so they never run in parallel.
static SERIAL: Mutex<()> = Mutex::new(());

const WAIT: Duration = Duration::from_secs(5);

#[derive(Clone, Default)]
struct Events(Arc<(Mutex<Vec<AudioEvent>>, Condvar)>);

impl Events {
    fn sink(&self) -> EventSink {
        let events = self.clone();
        Box::new(move |event| {
            let (list, signal) = &*events.0;
            list.lock().unwrap().push(event);
            signal.notify_all();
        })
    }

    fn all(&self) -> Vec<AudioEvent> {
        self.0.0.lock().unwrap().clone()
    }

    fn clear(&self) {
        self.0.0.lock().unwrap().clear();
    }

    /// Waits until some event satisfies `matches`, returning it.
    fn wait_for(&self, timeout: Duration, mut matches: impl FnMut(&AudioEvent) -> bool) -> Option<AudioEvent> {
        let (list, signal) = &*self.0;
        let deadline = Instant::now() + timeout;
        let mut guard = list.lock().unwrap();

        loop {
            if let Some(found) = guard.iter().find(|e| matches(e)) {
                return Some(found.clone());
            }

            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return None;
            }
            guard = signal.wait_timeout(guard, left).unwrap().0;
        }
    }

    fn wait_for_snapshot(
        &self,
        timeout: Duration,
        mut matches: impl FnMut(&AudioSnapshot) -> bool,
    ) -> Option<AudioSnapshot> {
        match self.wait_for(timeout, |e| matches!(e, AudioEvent::Snapshot(s) if matches(s))) {
            Some(AudioEvent::Snapshot(s)) => Some(s),
            _ => None,
        }
    }
}

fn start() -> (AudioEngine, Events, AudioSnapshot) {
    // DEEJ_LOG=debug shows the engine's own log while debugging a test
    let _ = tracing_subscriber::fmt()
        .with_env_filter(std::env::var("DEEJ_LOG").unwrap_or_else(|_| "warn".into()))
        .with_test_writer()
        .try_init();

    let events = Events::default();
    let engine = AudioEngine::start(events.sink()).unwrap();
    let snapshot = events.wait_for_snapshot(WAIT, |s| s.available).expect("audio should be available");
    (engine, events, snapshot)
}

/// A playback device other than the default if there is one, so changes aren't audible.
fn quiet_render_endpoint(snapshot: &AudioSnapshot) -> EndpointInfo {
    let render = || snapshot.endpoints.iter().filter(|e| e.flow == Flow::Render);
    render().find(|e| !e.is_default).or_else(|| render().next()).expect("a playback device").clone()
}

fn current(engine: &AudioEngine, events: &Events) -> AudioSnapshot {
    events.clear();
    engine.send(Command::RequestSnapshot);
    events.wait_for_snapshot(WAIT, |_| true).expect("snapshot")
}

/// Changes an endpoint's volume the way another app would (no deej event context).
fn set_endpoint_volume_externally(endpoint_id: &str, volume: f32) {
    let id = HSTRING::from(endpoint_id);
    std::thread::spawn(move || unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED).ok().unwrap();
        {
            let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).unwrap();
            let device = enumerator.GetDevice(&id).unwrap();
            let endpoint: IAudioEndpointVolume = device.Activate(CLSCTX_ALL, None).unwrap();
            endpoint.SetMasterVolumeLevelScalar(volume, std::ptr::null()).unwrap();
        }
        CoUninitialize();
    })
    .join()
    .unwrap();
}

fn nudged(volume: f32, by: f32) -> f32 {
    if volume > 0.5 { volume - by } else { volume + by }
}

#[test]
#[ignore = "needs real audio devices"]
fn starts_and_reports_devices_and_sessions() {
    let _serial = SERIAL.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let (engine, _events, snapshot) = start();

    assert!(snapshot.default_endpoint(Flow::Render).is_some(), "a default playback device");
    assert!(snapshot.endpoints.iter().all(|e| !e.name.is_empty() && !e.id.is_empty()));
    assert!(snapshot.sessions.iter().all(|s| !s.key.is_empty()));
    assert!(snapshot.sessions.iter().all(|s| snapshot.endpoint(&s.endpoint_id).is_some()));
    assert!(snapshot.sessions.iter().any(|s| s.is_system && s.key == "system"));

    engine.shutdown();
}

#[test]
#[ignore = "needs real audio devices"]
fn sets_endpoint_volume_and_mute_and_restores_them() {
    let _serial = SERIAL.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let (engine, events, snapshot) = start();
    let endpoint = quiet_render_endpoint(&snapshot);
    let target = || VolumeTarget::Endpoint(EndpointRef::Id(endpoint.id.clone()));
    let changed = ChangedTarget::Endpoint(endpoint.id.clone());

    let new_volume = nudged(endpoint.volume, 0.1);
    events.clear();
    engine.send(Command::SetVolume { target: target(), volume: new_volume });

    let event = events.wait_for(WAIT, |e| matches!(e, AudioEvent::VolumeChanged { target, .. } if *target == changed));
    assert_eq!(
        event,
        Some(AudioEvent::VolumeChanged {
            target: changed.clone(),
            volume: new_volume,
            muted: endpoint.muted,
            external: false
        })
    );

    // deej's own change must never be reported as an external one when Windows echoes it back
    std::thread::sleep(Duration::from_millis(500));
    assert!(!events.all().iter().any(|e| matches!(e, AudioEvent::VolumeChanged { external: true, .. })));

    engine.send(Command::SetMute { target: target(), muted: !endpoint.muted });
    assert!(
        events
            .wait_for(
                WAIT,
                |e| matches!(e, AudioEvent::VolumeChanged { muted, external: false, .. } if *muted != endpoint.muted)
            )
            .is_some()
    );

    engine.send(Command::SetMute { target: target(), muted: endpoint.muted });
    engine.send(Command::SetVolume { target: target(), volume: endpoint.volume });

    let after = current(&engine, &events);
    let restored = after.endpoint(&endpoint.id).unwrap();
    assert!((restored.volume - endpoint.volume).abs() < 0.001, "{} vs {}", restored.volume, endpoint.volume);
    assert_eq!(restored.muted, endpoint.muted);

    engine.shutdown();
}

#[test]
#[ignore = "needs real audio devices"]
fn default_device_target_resolves_to_the_current_default() {
    let _serial = SERIAL.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let (engine, events, snapshot) = start();
    let default = snapshot.default_endpoint(Flow::Render).unwrap().clone();
    let target = || VolumeTarget::Endpoint(EndpointRef::Default(Flow::Render));

    events.clear();
    engine.send(Command::SetVolume { target: target(), volume: nudged(default.volume, 0.01) });

    let event = events.wait_for(WAIT, |e| matches!(e, AudioEvent::VolumeChanged { .. }));
    assert!(
        matches!(&event, Some(AudioEvent::VolumeChanged { target: ChangedTarget::Endpoint(id), .. }) if *id == default.id),
        "{event:?}"
    );

    engine.send(Command::SetVolume { target: target(), volume: default.volume });
    let after = current(&engine, &events);
    assert!((after.endpoint(&default.id).unwrap().volume - default.volume).abs() < 0.001);

    engine.shutdown();
}

#[test]
#[ignore = "needs real audio devices"]
fn reports_volume_changes_made_outside_deej() {
    let _serial = SERIAL.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let (engine, events, snapshot) = start();
    let endpoint = quiet_render_endpoint(&snapshot);
    let changed = ChangedTarget::Endpoint(endpoint.id.clone());

    let new_volume = nudged(endpoint.volume, 0.2);
    events.clear();
    set_endpoint_volume_externally(&endpoint.id, new_volume);

    let event = events.wait_for(WAIT, |e| matches!(e, AudioEvent::VolumeChanged { target, .. } if *target == changed));
    match event {
        Some(AudioEvent::VolumeChanged { volume, external, .. }) => {
            assert!(external, "a change made by another app must be flagged external");
            assert!((volume - new_volume).abs() < 0.001, "{volume} vs {new_volume}");
        }
        other => panic!("no volume event: {other:?}"),
    }

    set_endpoint_volume_externally(&endpoint.id, endpoint.volume);
    engine.shutdown();
}

/// A PowerShell process playing silence in a loop: a real app with a real audio session, but inaudible.
struct SilentPlayer(Child);

impl SilentPlayer {
    fn start() -> SilentPlayer {
        const SCRIPT: &str = r#"
$rate = 8000; $bytes = $rate * 2
$ms = New-Object IO.MemoryStream; $w = New-Object IO.BinaryWriter($ms)
$w.Write([Text.Encoding]::ASCII.GetBytes('RIFF')); $w.Write([int](36 + $bytes))
$w.Write([Text.Encoding]::ASCII.GetBytes('WAVEfmt ')); $w.Write([int]16); $w.Write([int16]1); $w.Write([int16]1)
$w.Write([int]$rate); $w.Write([int]($rate * 2)); $w.Write([int16]2); $w.Write([int16]16)
$w.Write([Text.Encoding]::ASCII.GetBytes('data')); $w.Write([int]$bytes); $w.Write((New-Object byte[] $bytes))
$ms.Position = 0
$player = New-Object Media.SoundPlayer($ms); $player.PlayLooping(); Start-Sleep -Seconds 120
"#;
        let child =
            Process::new("powershell.exe").args(["-NoProfile", "-NonInteractive", "-Command", SCRIPT]).spawn().unwrap();
        SilentPlayer(child)
    }

    fn pid(&self) -> u32 {
        self.0.id()
    }
}

impl Drop for SilentPlayer {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
#[ignore = "needs real audio devices"]
fn discovers_new_sessions_through_notifications_and_drops_them_when_the_app_exits() {
    let _serial = SERIAL.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let (engine, events, _) = start();

    // no periodic re-scan: anything found has to come from Windows' notifications
    engine.send(Command::SetReconcileInterval(Duration::from_secs(600)));

    let player = SilentPlayer::start();
    let pid = player.pid();

    let snapshot = events
        .wait_for_snapshot(Duration::from_secs(10), |s| s.sessions.iter().any(|x| x.pid == pid))
        .expect("the new app's session should be reported without a re-scan");
    let session = snapshot.sessions.iter().find(|s| s.pid == pid).unwrap().clone();
    assert_eq!(session.key, "powershell.exe");
    assert!(session.exe_path.as_deref().is_some_and(|p| p.to_lowercase().ends_with("powershell.exe")));

    // Windows remembers each app's volume between runs, so move away from whatever it starts at, and put it back
    let original =
        std::env::var("DEEJ_TEST_RESTORE_SESSION_VOLUME").ok().and_then(|v| v.parse().ok()).unwrap_or(session.volume);
    let new_volume = nudged(session.volume, 0.25);

    events.clear();
    engine.send(Command::SetVolume { target: VolumeTarget::Session(session.id), volume: new_volume });
    assert!(
        events
            .wait_for(WAIT, |e| matches!(e, AudioEvent::VolumeChanged { target: ChangedTarget::Session(id), volume, external: false, .. } if *id == session.id && *volume == new_volume))
            .is_some()
    );

    engine.send(Command::SetVolume { target: VolumeTarget::Session(session.id), volume: original });
    let restored = current(&engine, &events).session(session.id).map(|s| s.volume);
    assert_eq!(restored, Some(original));

    events.clear();
    drop(player);

    let gone = events.wait_for_snapshot(Duration::from_secs(10), |s| s.session(session.id).is_none());
    assert!(gone.is_some(), "the session should disappear once the app exits");

    engine.shutdown();
}

#[test]
#[ignore = "needs real audio devices"]
fn starts_and_stops_repeatedly_without_leaking_threads() {
    let _serial = SERIAL.lock().unwrap_or_else(|poisoned| poisoned.into_inner());

    for _ in 0..5 {
        let (engine, _, _) = start();
        engine.shutdown();
    }
}
