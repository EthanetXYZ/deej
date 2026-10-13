//! An in-memory stand-in for the audio engine, for testing code that drives audio without real devices.
//!
//! It applies volume and mute commands to its snapshot with the same rules as the real engine (clamping, and
//! skipping a set when the volume is already exactly right), records every command, and lets tests simulate
//! external changes and device/session changes.

use std::sync::Mutex;

use crate::model::*;

pub struct FakeAudio {
    state: Mutex<FakeState>,
    sink: EventSink,
}

struct FakeState {
    snapshot: AudioSnapshot,
    commands: Vec<Command>,
    /// Volume changes actually applied (after the skip-if-equal rule), in order.
    applied: Vec<(ChangedTarget, f32)>,
}

impl FakeAudio {
    pub fn new(snapshot: AudioSnapshot, sink: EventSink) -> FakeAudio {
        FakeAudio { state: Mutex::new(FakeState { snapshot, commands: Vec::new(), applied: Vec::new() }), sink }
    }

    pub fn snapshot(&self) -> AudioSnapshot {
        self.state.lock().unwrap().snapshot.clone()
    }

    pub fn commands(&self) -> Vec<Command> {
        self.state.lock().unwrap().commands.clone()
    }

    pub fn applied(&self) -> Vec<(ChangedTarget, f32)> {
        self.state.lock().unwrap().applied.clone()
    }

    /// Replaces the devices and sessions, as if Windows reported a change.
    pub fn set_snapshot(&self, snapshot: AudioSnapshot) {
        self.state.lock().unwrap().snapshot = snapshot.clone();
        (self.sink)(AudioEvent::Snapshot(snapshot));
    }

    /// Simulates a volume change made outside deej (e.g. in the Windows volume mixer).
    pub fn external_change(&self, target: ChangedTarget, volume: f32, muted: bool) {
        {
            let mut state = self.state.lock().unwrap();
            if let Some((v, m)) = state.levels_mut(&target) {
                *v = volume;
                *m = muted;
            }
        }
        (self.sink)(AudioEvent::VolumeChanged { target, volume, muted, external: true });
    }
}

impl FakeState {
    fn resolve(&self, target: &VolumeTarget) -> Option<ChangedTarget> {
        match target {
            VolumeTarget::Session(id) => self.snapshot.session(*id).map(|s| ChangedTarget::Session(s.id)),
            VolumeTarget::Endpoint(EndpointRef::Id(id)) => {
                self.snapshot.endpoint(id).map(|e| ChangedTarget::Endpoint(e.id.clone()))
            }
            VolumeTarget::Endpoint(EndpointRef::Default(flow)) => {
                self.snapshot.default_endpoint(*flow).map(|e| ChangedTarget::Endpoint(e.id.clone()))
            }
        }
    }

    fn levels_mut(&mut self, target: &ChangedTarget) -> Option<(&mut f32, &mut bool)> {
        match target {
            ChangedTarget::Session(id) => {
                self.snapshot.sessions.iter_mut().find(|s| s.id == *id).map(|s| (&mut s.volume, &mut s.muted))
            }
            ChangedTarget::Endpoint(id) => {
                self.snapshot.endpoints.iter_mut().find(|e| e.id == *id).map(|e| (&mut e.volume, &mut e.muted))
            }
        }
    }

    /// Returns the event to emit, if anything changed.
    fn set_volume(&mut self, target: &VolumeTarget, volume: f32) -> Option<AudioEvent> {
        let volume = clamp_volume(volume);
        let changed = self.resolve(target)?;
        let (v, m) = self.levels_mut(&changed)?;

        if *v == volume {
            return None;
        }

        *v = volume;
        let muted = *m;
        self.applied.push((changed.clone(), volume));
        Some(AudioEvent::VolumeChanged { target: changed, volume, muted, external: false })
    }

    fn set_default(&mut self, id: &str) -> Option<AudioEvent> {
        let flow = self.snapshot.endpoint(id)?.flow;
        for e in self.snapshot.endpoints.iter_mut().filter(|e| e.flow == flow) {
            e.is_default = e.id == id;
        }
        Some(AudioEvent::Snapshot(self.snapshot.clone()))
    }

    fn set_mute(&mut self, target: &VolumeTarget, muted: bool) -> Option<AudioEvent> {
        let changed = self.resolve(target)?;
        let (v, m) = self.levels_mut(&changed)?;

        if *m == muted {
            return None;
        }

        *m = muted;
        Some(AudioEvent::VolumeChanged { target: changed, volume: *v, muted, external: false })
    }
}

impl AudioControl for FakeAudio {
    fn send(&self, command: Command) {
        let events: Vec<AudioEvent> = {
            let mut state = self.state.lock().unwrap();
            state.commands.push(command.clone());

            match &command {
                Command::SetVolume { target, volume } => state.set_volume(target, *volume).into_iter().collect(),
                Command::SetVolumes(sets) => sets.iter().filter_map(|(t, v)| state.set_volume(t, *v)).collect(),
                Command::SetMute { target, muted } => state.set_mute(target, *muted).into_iter().collect(),
                Command::RequestSnapshot | Command::Rescan => vec![AudioEvent::Snapshot(state.snapshot.clone())],
                Command::SetDefaultEndpoint { id } => state.set_default(id).into_iter().collect(),
                Command::SetReconcileInterval(_) => Vec::new(),
            }
        };

        for event in events {
            (self.sink)(event);
        }
    }
}

pub use deej_core::audio::sample_snapshot;

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    fn fake() -> (FakeAudio, Arc<Mutex<Vec<AudioEvent>>>) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink = events.clone();
        (FakeAudio::new(sample_snapshot(), Box::new(move |e| sink.lock().unwrap().push(e))), events)
    }

    #[test]
    fn sets_volumes_and_skips_unchanged_ones() {
        let (audio, events) = fake();

        audio.send(Command::SetVolumes(vec![
            (VolumeTarget::Session(2), 0.3),
            (VolumeTarget::Session(3), 1.0), // already 1.0
            (VolumeTarget::Endpoint(EndpointRef::Default(Flow::Render)), 1.5),
        ]));

        assert_eq!(
            audio.applied(),
            vec![(ChangedTarget::Session(2), 0.3), (ChangedTarget::Endpoint("speakers".into()), 1.0)]
        );
        assert_eq!(events.lock().unwrap().len(), 2);
        assert_eq!(audio.snapshot().session(3).unwrap().volume, 1.0);
    }

    #[test]
    fn default_endpoint_follows_the_snapshot() {
        let (audio, _) = fake();

        let mut changed = sample_snapshot();
        changed.endpoints[0].is_default = false;
        changed.endpoints[1].is_default = true;
        audio.set_snapshot(changed);

        audio.send(Command::SetVolume {
            target: VolumeTarget::Endpoint(EndpointRef::Default(Flow::Render)),
            volume: 0.2,
        });
        assert_eq!(audio.snapshot().endpoint("headset").unwrap().volume, 0.2);
    }

    #[test]
    fn external_changes_are_flagged() {
        let (audio, events) = fake();
        audio.external_change(ChangedTarget::Session(5), 0.1, true);

        let last = events.lock().unwrap().last().cloned().unwrap();
        assert_eq!(
            last,
            AudioEvent::VolumeChanged { target: ChangedTarget::Session(5), volume: 0.1, muted: true, external: true }
        );
        assert!(audio.snapshot().session(5).unwrap().muted);
    }

    #[test]
    fn unknown_targets_are_ignored() {
        let (audio, events) = fake();
        audio.send(Command::SetVolume { target: VolumeTarget::Session(99), volume: 0.5 });
        audio.send(Command::SetMute { target: VolumeTarget::Endpoint(EndpointRef::Id("nope".into())), muted: true });
        assert!(events.lock().unwrap().is_empty());
    }
}
