//! The Windows Core Audio engine.
//!
//! One dedicated thread (`deej-audio`, COM multithreaded apartment) owns every Core Audio object for its whole
//! life. Commands from deej and notifications from Windows both arrive as [`Msg`]s on one queue, so no Core Audio
//! object is ever used from two threads, or after it was released. This replaces the locks the Go version needed
//! after real crashes (see ARCHITECTURE.md D1).
//!
//! Discovery is event-driven (device, session and volume notifications), with a periodic re-scan as a safety net
//! for anything a notification missed. If audio is unavailable (no devices over Remote Desktop, or the audio
//! service restarting) the engine reports it and keeps retrying.

mod callbacks;
mod policy;
mod util;

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use windows::Win32::Devices::FunctionDiscovery::{PKEY_Device_DeviceDesc, PKEY_Device_FriendlyName};
use windows::Win32::Foundation::S_OK;
use windows::Win32::Media::Audio::Endpoints::{IAudioEndpointVolume, IAudioEndpointVolumeCallback};
use windows::Win32::Media::Audio::{
    AudioSessionStateActive, AudioSessionStateExpired, DEVICE_STATE_ACTIVE, IAudioSessionControl2, IAudioSessionEvents,
    IAudioSessionManager2, IAudioSessionNotification, IMMDevice, IMMDeviceEnumerator, IMMEndpoint,
    IMMNotificationClient, ISimpleAudioVolume, MMDeviceEnumerator, eAll, eCapture, eConsole, eRender,
};
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize, STGM_READ,
};
use windows_core::{GUID, Interface};

use crate::model::*;
use util::{Failure, classify, take_pwstr};

/// Passed with every volume change deej makes, so its own changes can be told apart from the user's.
/// The same GUID the Go deej used.
pub const DEEJ_EVENT_CONTEXT: GUID = GUID::from_u128(0x1ec920a1_7db8_44ba_9779_e5d28ed9f330);

pub const DEFAULT_RECONCILE_INTERVAL: Duration = Duration::from_secs(10);
const UNAVAILABLE_RETRY_INTERVAL: Duration = Duration::from_secs(5);

/// Messages for the engine thread: commands from deej, notifications from Windows.
pub(crate) enum Msg {
    /// A command, with when it was sent (for latency measurements).
    Command(Command, Instant),
    Shutdown,
    DevicesChanged,
    DefaultChanged {
        flow: Flow,
        endpoint_id: Option<String>,
    },
    EndpointVolume {
        endpoint_id: String,
        volume: f32,
        muted: bool,
        ours: bool,
    },
    SessionsChanged {
        endpoint_id: String,
    },
    SessionChanged {
        session: SessionId,
    },
    SessionVolume {
        session: SessionId,
        volume: f32,
        muted: bool,
        ours: bool,
    },
    SessionGone {
        session: SessionId,
    },
}

/// Handle to the running engine. Dropping it stops the engine thread.
pub struct AudioEngine {
    tx: Sender<Msg>,
    thread: Option<JoinHandle<()>>,
}

impl AudioEngine {
    /// Starts the engine thread. The first [`AudioEvent::Snapshot`] arrives once devices have been enumerated.
    pub fn start(sink: EventSink) -> std::io::Result<AudioEngine> {
        let (tx, rx) = mpsc::channel();
        let engine_tx = tx.clone();

        let thread = std::thread::Builder::new().name("deej-audio".into()).spawn(move || run(rx, engine_tx, sink))?;

        Ok(AudioEngine { tx, thread: Some(thread) })
    }

    /// Stops the engine, releasing every Core Audio object and unregistering all notifications.
    pub fn shutdown(mut self) {
        self.stop();
    }

    fn stop(&mut self) {
        if let Some(thread) = self.thread.take() {
            let _ = self.tx.send(Msg::Shutdown);
            let _ = thread.join();
        }
    }
}

impl Drop for AudioEngine {
    fn drop(&mut self) {
        self.stop();
    }
}

impl AudioControl for AudioEngine {
    fn send(&self, command: Command) {
        let _ = self.tx.send(Msg::Command(command, Instant::now()));
    }
}

fn run(rx: Receiver<Msg>, tx: Sender<Msg>, sink: EventSink) {
    // SAFETY: initializes COM for this thread, balanced by CoUninitialize below
    let init = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    if init.is_err() {
        tracing::error!(error = %init.message(), "Couldn't initialize COM; audio control is unavailable");
        sink(AudioEvent::Snapshot(AudioSnapshot::default()));
        return;
    }

    {
        let mut engine = Engine::new(tx, sink);
        engine.connect();
        engine.publish_snapshot();

        let mut next_reconcile = Instant::now() + engine.retry_or_reconcile_interval();

        loop {
            let timeout = next_reconcile.saturating_duration_since(Instant::now());

            match rx.recv_timeout(timeout) {
                Ok(first) => {
                    let mut batch = vec![first];
                    batch.extend(rx.try_iter());

                    if !engine.handle_batch(batch) {
                        break;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }

            if Instant::now() >= next_reconcile {
                engine.reconcile();
                next_reconcile = Instant::now() + engine.retry_or_reconcile_interval();
            }
        }

        tracing::debug!("Audio engine stopping");
        // engine drops here: unregisters every callback and releases every object, before COM goes away
    }

    // SAFETY: balances the successful CoInitializeEx above, after every COM object was released
    unsafe { CoUninitialize() };
}

struct Endpoint {
    info: EndpointInfo,
    volume: IAudioEndpointVolume,
    volume_callback: IAudioEndpointVolumeCallback,
    /// Playback devices only: their app sessions.
    sessions: Option<(IAudioSessionManager2, IAudioSessionNotification)>,
}

impl Drop for Endpoint {
    fn drop(&mut self) {
        // SAFETY: unregistering callbacks we registered, on the thread that owns the objects
        unsafe {
            let _ = self.volume.UnregisterControlChangeNotify(&self.volume_callback);
            if let Some((manager, notifier)) = &self.sessions {
                let _ = manager.UnregisterSessionNotification(notifier);
            }
        }
    }
}

struct Session {
    info: SessionInfo,
    control: IAudioSessionControl2,
    volume: ISimpleAudioVolume,
    events: IAudioSessionEvents,
}

impl Drop for Session {
    fn drop(&mut self) {
        // SAFETY: unregistering a callback we registered, on the thread that owns the session
        unsafe {
            let _ = self.control.UnregisterAudioSessionNotification(&self.events);
        }
    }
}

struct Engine {
    tx: Sender<Msg>,
    sink: EventSink,

    enumerator: Option<IMMDeviceEnumerator>,
    device_notifier: Option<IMMNotificationClient>,

    /// In Windows' enumeration order.
    endpoints: Vec<Endpoint>,
    sessions: HashMap<SessionId, Session>,

    /// Session instance identifier → id, so a session keeps its id across re-scans.
    session_ids: HashMap<String, SessionId>,
    next_session_id: SessionId,

    default_render: Option<String>,
    default_capture: Option<String>,

    reconcile_interval: Duration,
    unavailable_reason: Option<String>,

    /// Set when devices or sessions were added, removed or renamed; a snapshot goes out at the end of the batch.
    structure_changed: bool,
}

/// Re-scans requested while handling a batch of messages, run before the next command and at the end.
#[derive(Default)]
struct Pending {
    devices: bool,
    endpoints: HashSet<String>,
}

impl Engine {
    fn new(tx: Sender<Msg>, sink: EventSink) -> Engine {
        Engine {
            tx,
            sink,
            enumerator: None,
            device_notifier: None,
            endpoints: Vec::new(),
            sessions: HashMap::new(),
            session_ids: HashMap::new(),
            next_session_id: 1,
            default_render: None,
            default_capture: None,
            reconcile_interval: DEFAULT_RECONCILE_INTERVAL,
            unavailable_reason: Some("not connected yet".into()),
            structure_changed: true,
        }
    }

    fn available(&self) -> bool {
        self.unavailable_reason.is_none()
    }

    fn retry_or_reconcile_interval(&self) -> Duration {
        if self.available() { self.reconcile_interval } else { UNAVAILABLE_RETRY_INTERVAL }
    }

    // ---- connection --------------------------------------------------------------------------------------

    /// Creates the device enumerator, registers for device notifications and scans everything.
    fn connect(&mut self) {
        if let Err(err) = self.try_connect() {
            self.disconnect(format!("Windows audio isn't available: {}", err.message()));
            return;
        }

        if let Err(err) = self.scan_devices() {
            self.handle_failure("scanning audio devices", &err);
        }
    }

    fn try_connect(&mut self) -> windows_core::Result<()> {
        if self.enumerator.is_some() {
            return Ok(());
        }

        // SAFETY: COM is initialized on this thread
        let enumerator: IMMDeviceEnumerator = unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)? };
        let notifier = callbacks::device_notifier(self.tx.clone());

        // SAFETY: registering our own callback object; it's unregistered in `disconnect`
        unsafe { enumerator.RegisterEndpointNotificationCallback(&notifier)? };

        self.enumerator = Some(enumerator);
        self.device_notifier = Some(notifier);
        Ok(())
    }

    /// Drops every Core Audio object. The engine retries on its own schedule.
    fn disconnect(&mut self, reason: String) {
        tracing::warn!(%reason, "Audio unavailable, will keep retrying");

        self.sessions.clear();
        self.endpoints.clear();

        if let (Some(enumerator), Some(notifier)) = (&self.enumerator, &self.device_notifier) {
            // SAFETY: unregistering the callback registered in try_connect
            unsafe {
                let _ = enumerator.UnregisterEndpointNotificationCallback(notifier);
            }
        }

        self.enumerator = None;
        self.device_notifier = None;
        self.default_render = None;
        self.default_capture = None;
        self.unavailable_reason = Some(reason);
        self.structure_changed = true;
    }

    fn handle_failure(&mut self, what: &str, err: &windows_core::Error) -> Failure {
        let failure = classify(err);
        match failure {
            Failure::ServiceGone => self.disconnect(format!("{what} failed: {}", err.message())),
            Failure::DeviceGone => tracing::debug!(what, error = %err.message(), "Audio device went away"),
            Failure::Other => tracing::warn!(what, error = %err.message(), "Audio call failed"),
        }
        failure
    }

    /// The periodic safety net: reconnect if unavailable, otherwise re-scan and fix up anything missed.
    fn reconcile(&mut self) {
        if !self.available() {
            self.connect();
        } else if let Err(err) = self.scan_devices() {
            self.handle_failure("re-scanning audio devices", &err);
        }

        self.publish_snapshot();
    }

    // ---- messages ----------------------------------------------------------------------------------------

    /// Handles a batch of queued messages. Returns false to stop the engine.
    fn handle_batch(&mut self, batch: Vec<Msg>) -> bool {
        let mut pending = Pending::default();

        for msg in batch {
            match msg {
                Msg::Shutdown => return false,

                Msg::Command(command, sent) => {
                    self.run_pending(&mut pending);
                    let started = Instant::now();
                    let is_volume = matches!(command, Command::SetVolumes(_) | Command::SetVolume { .. });
                    self.run_command(command);
                    if is_volume {
                        let done = Instant::now();
                        tracing::debug!(
                            queued_us = started.duration_since(sent).as_micros() as u64,
                            apply_us = done.duration_since(started).as_micros() as u64,
                            "Command done"
                        );
                    }
                }

                Msg::DevicesChanged => pending.devices = true,
                Msg::SessionsChanged { endpoint_id } => {
                    pending.endpoints.insert(endpoint_id);
                }

                Msg::DefaultChanged { flow, endpoint_id } => self.set_default(flow, endpoint_id),

                Msg::EndpointVolume { endpoint_id, volume, muted, ours } => {
                    self.on_endpoint_volume(&endpoint_id, volume, muted, ours)
                }
                Msg::SessionVolume { session, volume, muted, ours } => {
                    self.on_session_volume(session, volume, muted, ours)
                }

                Msg::SessionChanged { session } => self.refresh_session(session),
                Msg::SessionGone { session } => self.remove_session(session),
            }
        }

        self.run_pending(&mut pending);
        self.publish_snapshot();
        true
    }

    fn run_pending(&mut self, pending: &mut Pending) {
        if std::mem::take(&mut pending.devices) {
            pending.endpoints.clear(); // a device scan covers every endpoint's sessions
            if !self.available() {
                self.connect();
            } else if let Err(err) = self.scan_devices() {
                self.handle_failure("scanning audio devices", &err);
            }
        }

        for endpoint_id in std::mem::take(&mut pending.endpoints) {
            let mut names = None;
            if let Err(err) = self.scan_sessions(&endpoint_id, &mut names) {
                if self.handle_failure("scanning audio sessions", &err) == Failure::DeviceGone {
                    pending.devices = true;
                }
            }
        }

        if pending.devices {
            self.run_pending(pending);
        }
    }

    fn run_command(&mut self, command: Command) {
        match command {
            Command::SetVolume { target, volume } => self.set_volume(&target, volume),
            Command::SetVolumes(sets) => {
                for (target, volume) in sets {
                    self.set_volume(&target, volume);
                }
            }
            Command::SetMute { target, muted } => self.set_mute(&target, muted),
            Command::Rescan => {
                if !self.available() {
                    self.connect();
                } else if let Err(err) = self.scan_devices() {
                    self.handle_failure("re-scanning audio devices", &err);
                }
                self.structure_changed = true;
            }
            Command::SetDefaultEndpoint { id } => match policy::set_default_endpoint(&id) {
                // Windows reports the change through the usual default-device notification
                Ok(()) => tracing::info!(endpoint = %id, "Default device changed"),
                Err(err) => tracing::warn!(endpoint = %id, error = %err, "Couldn't change the default device"),
            },
            Command::RequestSnapshot => self.structure_changed = true,
            Command::SetReconcileInterval(interval) => {
                self.reconcile_interval = interval.max(Duration::from_millis(500));
            }
        }
    }

    fn publish_snapshot(&mut self) {
        if !std::mem::take(&mut self.structure_changed) {
            return;
        }

        let mut sessions: Vec<SessionInfo> = self.sessions.values().map(|s| s.info.clone()).collect();
        sessions.sort_by_key(|s| s.id);

        let snapshot = AudioSnapshot {
            available: self.available(),
            endpoints: self.endpoints.iter().map(|e| e.info.clone()).collect(),
            sessions,
        };

        tracing::debug!(
            available = snapshot.available,
            endpoints = snapshot.endpoints.len(),
            sessions = snapshot.sessions.len(),
            "Audio snapshot"
        );

        (self.sink)(AudioEvent::Snapshot(snapshot));
    }

    // ---- devices -----------------------------------------------------------------------------------------

    fn scan_devices(&mut self) -> windows_core::Result<()> {
        let Some(enumerator) = self.enumerator.clone() else { return Ok(()) };

        let default_id = |flow| -> Option<String> {
            // SAFETY: plain query on our enumerator; an error just means there's no default device of that kind
            unsafe { enumerator.GetDefaultAudioEndpoint(flow, eConsole).and_then(|d| d.GetId()).ok().map(take_pwstr) }
        };
        let default_render = default_id(eRender);
        let default_capture = default_id(eCapture);

        // SAFETY: enumerating active endpoints on our enumerator
        let collection = unsafe { enumerator.EnumAudioEndpoints(eAll, DEVICE_STATE_ACTIVE)? };
        let count = unsafe { collection.GetCount()? };

        let mut seen = Vec::with_capacity(count as usize);
        for index in 0..count {
            // SAFETY: index is within the collection's count
            let device = match unsafe { collection.Item(index) } {
                Ok(device) => device,
                Err(err) => {
                    tracing::debug!(index, error = %err.message(), "Skipping audio device");
                    continue;
                }
            };

            match self.add_or_update_endpoint(&device) {
                Ok(id) => seen.push(id),
                Err(err) => {
                    if self.handle_failure("reading an audio device", &err) == Failure::ServiceGone {
                        return Ok(());
                    }
                }
            }
        }

        // devices that are gone (unplugged or disabled), and their sessions
        let before = self.endpoints.len();
        self.endpoints.retain(|e| seen.contains(&e.info.id));
        if self.endpoints.len() != before {
            self.structure_changed = true;
            let live: HashSet<&String> = self.endpoints.iter().map(|e| &e.info.id).collect();
            self.sessions.retain(|_, s| live.contains(&s.info.endpoint_id));
        }

        // put endpoints in Windows' order
        self.endpoints.sort_by_key(|e| seen.iter().position(|id| *id == e.info.id));

        self.set_default(Flow::Render, default_render);
        self.set_default(Flow::Capture, default_capture);

        let mut names = None;
        let render_ids: Vec<String> =
            self.endpoints.iter().filter(|e| e.sessions.is_some()).map(|e| e.info.id.clone()).collect();
        for id in render_ids {
            if let Err(err) = self.scan_sessions(&id, &mut names) {
                if self.handle_failure("scanning audio sessions", &err) == Failure::ServiceGone {
                    return Ok(());
                }
            }
        }

        let was_unavailable = self.unavailable_reason.take();
        if self.endpoints.is_empty() {
            self.unavailable_reason = Some("Windows reports no active audio devices".into());
        }
        if was_unavailable.is_some() != self.unavailable_reason.is_some() {
            self.structure_changed = true;
            if self.available() {
                tracing::info!(endpoints = self.endpoints.len(), sessions = self.sessions.len(), "Audio available");
            }
        }

        Ok(())
    }

    /// Adds a newly seen device, or refreshes one we already have. Returns its id.
    fn add_or_update_endpoint(&mut self, device: &IMMDevice) -> windows_core::Result<String> {
        // SAFETY: reading properties of a device the enumerator just returned
        let (id, name, description, flow) = unsafe {
            let id = take_pwstr(device.GetId()?);
            let store = device.OpenPropertyStore(STGM_READ)?;
            let name = store.GetValue(&PKEY_Device_FriendlyName).map(|v| v.to_string()).unwrap_or_default();
            let description = store.GetValue(&PKEY_Device_DeviceDesc).map(|v| v.to_string()).unwrap_or_default();
            let flow =
                if device.cast::<IMMEndpoint>()?.GetDataFlow()? == eCapture { Flow::Capture } else { Flow::Render };
            (id, name, description, flow)
        };

        if let Some(existing) = self.endpoints.iter_mut().find(|e| e.info.id == id) {
            if existing.info.name != name || existing.info.description != description {
                existing.info.name = name;
                existing.info.description = description;
                self.structure_changed = true;
            }

            // catch volume changes a notification missed
            // SAFETY: reading the endpoint's own volume interface
            let (volume, muted) =
                unsafe { (existing.volume.GetMasterVolumeLevelScalar()?, existing.volume.GetMute()?.as_bool()) };
            let id = existing.info.id.clone();
            self.on_endpoint_volume(&id, volume, muted, false);
            return Ok(id);
        }

        // SAFETY: activating interfaces on a live device and registering our own callbacks (undone in Drop)
        let endpoint = unsafe {
            let volume: IAudioEndpointVolume = device.Activate(CLSCTX_ALL, None)?;
            let callback = callbacks::endpoint_volume_notifier(self.tx.clone(), id.clone());
            volume.RegisterControlChangeNotify(&callback)?;

            let info = EndpointInfo {
                id: id.clone(),
                name,
                description,
                flow,
                is_default: false,
                volume: volume.GetMasterVolumeLevelScalar()?,
                muted: volume.GetMute()?.as_bool(),
            };

            let mut endpoint = Endpoint { info, volume, volume_callback: callback, sessions: None };

            if flow == Flow::Render {
                let manager: IAudioSessionManager2 = device.Activate(CLSCTX_ALL, None)?;
                let notifier = callbacks::session_notifier(self.tx.clone(), id.clone());
                manager.RegisterSessionNotification(&notifier)?;
                endpoint.sessions = Some((manager, notifier));
            }

            endpoint
        };

        tracing::info!(name = %endpoint.info.name, flow = ?endpoint.info.flow, "Audio device added");
        self.endpoints.push(endpoint);
        self.structure_changed = true;
        Ok(id)
    }

    fn set_default(&mut self, flow: Flow, endpoint_id: Option<String>) {
        let current = match flow {
            Flow::Render => &mut self.default_render,
            Flow::Capture => &mut self.default_capture,
        };

        if *current != endpoint_id {
            tracing::info!(?flow, endpoint = ?endpoint_id, "Default audio device changed");
            *current = endpoint_id.clone();
        }

        for endpoint in self.endpoints.iter_mut().filter(|e| e.info.flow == flow) {
            let is_default = endpoint_id.as_deref() == Some(endpoint.info.id.as_str());
            if endpoint.info.is_default != is_default {
                endpoint.info.is_default = is_default;
                self.structure_changed = true;
            }
        }
    }

    // ---- sessions ----------------------------------------------------------------------------------------

    /// Re-enumerates one playback device's sessions: adds new ones, refreshes known ones, drops vanished ones.
    /// `names` caches the process list for the duration of a scan.
    fn scan_sessions(
        &mut self,
        endpoint_id: &str,
        names: &mut Option<HashMap<u32, String>>,
    ) -> windows_core::Result<()> {
        let Some(manager) = self
            .endpoints
            .iter()
            .find(|e| e.info.id == endpoint_id)
            .and_then(|e| e.sessions.as_ref())
            .map(|(m, _)| m.clone())
        else {
            return Ok(());
        };

        // SAFETY: enumerating sessions on a live session manager. Enumerating also makes Windows start sending
        // session-created notifications for this device (it doesn't until the enumerator was used once)
        let enumerator = unsafe { manager.GetSessionEnumerator()? };
        let count = unsafe { enumerator.GetCount()? };

        let mut seen = HashSet::new();
        for index in 0..count {
            // SAFETY: index is within the enumerator's count
            let control = match unsafe { enumerator.GetSession(index) }.and_then(|c| c.cast::<IAudioSessionControl2>())
            {
                Ok(control) => control,
                Err(err) => {
                    tracing::debug!(index, error = %err.message(), "Skipping audio session");
                    continue;
                }
            };

            match self.add_or_update_session(endpoint_id, control, names) {
                Ok(Some(id)) => {
                    seen.insert(id);
                }
                Ok(None) => {}
                Err(err) => {
                    if classify(&err) == Failure::ServiceGone {
                        return Err(err);
                    }
                    tracing::debug!(error = %err.message(), "Skipping audio session");
                }
            }
        }

        let gone: Vec<SessionId> = self
            .sessions
            .values()
            .filter(|s| s.info.endpoint_id == endpoint_id && !seen.contains(&s.info.id))
            .map(|s| s.info.id)
            .collect();
        for id in gone {
            self.remove_session(id);
        }

        Ok(())
    }

    fn add_or_update_session(
        &mut self,
        endpoint_id: &str,
        control: IAudioSessionControl2,
        names: &mut Option<HashMap<u32, String>>,
    ) -> windows_core::Result<Option<SessionId>> {
        // SAFETY: reading a session control Windows just handed us
        let (instance, state) = unsafe { (take_pwstr(control.GetSessionInstanceIdentifier()?), control.GetState()?) };

        if state == AudioSessionStateExpired {
            return Ok(None);
        }

        let state = if state == AudioSessionStateActive { SessionState::Active } else { SessionState::Inactive };

        if let Some(&id) = self.session_ids.get(&instance) {
            // an idle session can outlive its process for a while; drop it as soon as the process is gone
            // (the Go deej skipped such sessions on every scan)
            let process_gone = self.sessions.get(&id).is_some_and(|s| {
                state == SessionState::Inactive
                    && !s.info.is_system
                    && !names.get_or_insert_with(util::process_names).contains_key(&s.info.pid)
            });
            if process_gone {
                self.remove_session(id);
                return Ok(None);
            }

            if let Some(session) = self.sessions.get_mut(&id) {
                if session.info.state != state {
                    session.info.state = state;
                    self.structure_changed = true;
                }

                // SAFETY: reading the session's own volume interface
                let (volume, muted) =
                    unsafe { (session.volume.GetMasterVolume()?, session.volume.GetMute()?.as_bool()) };
                self.on_session_volume(id, volume, muted, false);
                return Ok(Some(id));
            }
        }

        // SAFETY: querying a live session; GetProcessId fails for some multi-process sessions, which then count as
        // system sounds only if Windows says so
        let (pid, is_system) =
            unsafe { (control.GetProcessId().unwrap_or(0), control.IsSystemSoundsSession() == S_OK) };

        let process_name = if is_system {
            String::new()
        } else {
            let table = names.get_or_insert_with(util::process_names);
            match table.get(&pid) {
                Some(name) => name.clone(),
                None => {
                    // the process already exited; Windows cleans the session up later (the Go deej skipped these too)
                    tracing::debug!(pid, "Skipping session of an exited process");
                    return Ok(None);
                }
            }
        };

        let id = *self.session_ids.entry(instance).or_insert_with(|| {
            let id = self.next_session_id;
            self.next_session_id += 1;
            id
        });

        // SAFETY: querying the session's volume interface and registering our own callback (undone in Drop)
        let session = unsafe {
            let volume: ISimpleAudioVolume = control.cast()?;
            let events = callbacks::session_events(self.tx.clone(), id);
            control.RegisterAudioSessionNotification(&events)?;

            let info = SessionInfo {
                id,
                endpoint_id: endpoint_id.to_string(),
                pid,
                key: session_key(&process_name, is_system),
                process_name,
                display_name: control.GetDisplayName().map(take_pwstr).unwrap_or_default(),
                exe_path: util::exe_path(pid),
                is_system,
                state,
                volume: volume.GetMasterVolume()?,
                muted: volume.GetMute()?.as_bool(),
            };

            Session { info, control, volume, events }
        };

        tracing::debug!(key = %session.info.key, pid, id, "Audio session added");
        self.sessions.insert(id, session);
        self.structure_changed = true;
        Ok(Some(id))
    }

    fn refresh_session(&mut self, id: SessionId) {
        let Some(session) = self.sessions.get_mut(&id) else { return };

        // SAFETY: reading the session's own control interface
        let (display_name, state) = unsafe {
            (session.control.GetDisplayName().map(take_pwstr).unwrap_or_default(), session.control.GetState())
        };

        match state {
            Ok(s) if s == AudioSessionStateExpired => self.remove_session(id),
            Ok(s) => {
                let state = if s == AudioSessionStateActive { SessionState::Active } else { SessionState::Inactive };
                if session.info.state != state || session.info.display_name != display_name {
                    session.info.state = state;
                    session.info.display_name = display_name;
                    self.structure_changed = true;
                }
            }
            Err(_) => self.remove_session(id),
        }
    }

    fn remove_session(&mut self, id: SessionId) {
        if let Some(session) = self.sessions.remove(&id) {
            tracing::debug!(key = %session.info.key, id, "Audio session removed");
            self.session_ids.retain(|_, v| *v != id);
            self.structure_changed = true;
        }
    }

    // ---- volume ------------------------------------------------------------------------------------------

    fn endpoint_index(&self, target: &EndpointRef) -> Option<usize> {
        let id = match target {
            EndpointRef::Id(id) => Some(id.as_str()),
            EndpointRef::Default(Flow::Render) => self.default_render.as_deref(),
            EndpointRef::Default(Flow::Capture) => self.default_capture.as_deref(),
        }?;

        self.endpoints.iter().position(|e| e.info.id == id)
    }

    fn set_volume(&mut self, target: &VolumeTarget, volume: f32) {
        let volume = clamp_volume(volume);

        let result = match target {
            VolumeTarget::Endpoint(endpoint) => {
                let Some(index) = self.endpoint_index(endpoint) else { return };
                let e = &mut self.endpoints[index];

                // SAFETY: calls on the endpoint's own volume interface, on the owning thread
                unsafe {
                    match e.volume.GetMasterVolumeLevelScalar() {
                        // already there: skip the call, like the Go deej did
                        Ok(current) if current == volume => return,
                        _ => e.volume.SetMasterVolumeLevelScalar(volume, &DEEJ_EVENT_CONTEXT).map(|()| {
                            e.info.volume = volume;
                            (ChangedTarget::Endpoint(e.info.id.clone()), e.info.muted)
                        }),
                    }
                }
            }
            VolumeTarget::Session(id) => {
                let Some(s) = self.sessions.get_mut(id) else { return };

                // SAFETY: calls on the session's own volume interface, on the owning thread
                unsafe {
                    let current = s.volume.GetMasterVolume();
                    tracing::trace!(
                        session = s.info.id,
                        ?current,
                        cached = s.info.volume,
                        requested = volume,
                        "Setting session volume"
                    );
                    match current {
                        Ok(current) if current == volume => return,
                        _ => s.volume.SetMasterVolume(volume, &DEEJ_EVENT_CONTEXT).map(|()| {
                            s.info.volume = volume;
                            (ChangedTarget::Session(s.info.id), s.info.muted)
                        }),
                    }
                }
            }
        };

        match result {
            Ok((target, muted)) => (self.sink)(AudioEvent::VolumeChanged { target, volume, muted, external: false }),
            Err(err) => self.on_set_failed(target, &err),
        }
    }

    fn set_mute(&mut self, target: &VolumeTarget, muted: bool) {
        let result = match target {
            VolumeTarget::Endpoint(endpoint) => {
                let Some(index) = self.endpoint_index(endpoint) else { return };
                let e = &mut self.endpoints[index];

                // SAFETY: call on the endpoint's own volume interface, on the owning thread
                unsafe { e.volume.SetMute(muted, &DEEJ_EVENT_CONTEXT) }.map(|()| {
                    e.info.muted = muted;
                    (ChangedTarget::Endpoint(e.info.id.clone()), e.info.volume)
                })
            }
            VolumeTarget::Session(id) => {
                let Some(s) = self.sessions.get_mut(id) else { return };

                // SAFETY: call on the session's own volume interface, on the owning thread
                unsafe { s.volume.SetMute(muted, &DEEJ_EVENT_CONTEXT) }.map(|()| {
                    s.info.muted = muted;
                    (ChangedTarget::Session(s.info.id), s.info.volume)
                })
            }
        };

        match result {
            Ok((target, volume)) => (self.sink)(AudioEvent::VolumeChanged { target, volume, muted, external: false }),
            Err(err) => self.on_set_failed(target, &err),
        }
    }

    fn on_set_failed(&mut self, target: &VolumeTarget, err: &windows_core::Error) {
        if self.handle_failure("setting a volume", err) == Failure::DeviceGone {
            // the device or session is gone; a re-scan brings the state up to date (the Go deej did the same)
            if let VolumeTarget::Session(id) = target {
                self.remove_session(*id);
            }
            if let Err(err) = self.scan_devices() {
                self.handle_failure("re-scanning audio devices", &err);
            }
        }
    }

    /// A volume notification (or a value read during a scan), reported if it differs from what we know.
    ///
    /// Notifications for deej's own changes (`ours`) are ignored: the new value was recorded and reported when the
    /// change was made, and Windows delivers these echoes late. Applying one could overwrite a newer value, e.g.
    /// set 75% then 100%, and the 75% echo arrives after the second call.
    fn on_endpoint_volume(&mut self, endpoint_id: &str, volume: f32, muted: bool, ours: bool) {
        if ours {
            return;
        }

        let Some(e) = self.endpoints.iter_mut().find(|e| e.info.id == endpoint_id) else { return };

        if e.info.volume == volume && e.info.muted == muted {
            return;
        }

        e.info.volume = volume;
        e.info.muted = muted;
        let target = ChangedTarget::Endpoint(e.info.id.clone());
        (self.sink)(AudioEvent::VolumeChanged { target, volume, muted, external: true });
    }

    fn on_session_volume(&mut self, id: SessionId, volume: f32, muted: bool, ours: bool) {
        if ours {
            return;
        }

        let Some(s) = self.sessions.get_mut(&id) else { return };

        if s.info.volume == volume && s.info.muted == muted {
            return;
        }

        s.info.volume = volume;
        s.info.muted = muted;
        (self.sink)(AudioEvent::VolumeChanged { target: ChangedTarget::Session(id), volume, muted, external: true });
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.sessions.clear();
        self.endpoints.clear();

        if let (Some(enumerator), Some(notifier)) = (&self.enumerator, &self.device_notifier) {
            // SAFETY: unregistering the callback registered in try_connect, before COM is uninitialized
            unsafe {
                let _ = enumerator.UnregisterEndpointNotificationCallback(notifier);
            }
        }
    }
}
