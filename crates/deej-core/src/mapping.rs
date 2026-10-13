//! Which volumes a slider controls: the original deej's target rules (`session_map.go`), as pure functions.
//!
//! | Target | Controls |
//! |---|---|
//! | `master` | the default playback device |
//! | `mic` | the default recording device |
//! | `system` | the system sounds session on every playback device |
//! | `chrome.exe` | every session of that executable, on every playback device (exact name, any case) |
//! | `Speakers (Realtek Audio)` | that device's volume, by friendly name (any case) |
//! | `deej.current` | the foreground app, including child windows of other processes (UWP hosts, launchers) |
//! | `deej.unmapped` | every app session no slider names, except `unmapped_exclude` |
//!
//! One deliberate difference (COMPATIBILITY_MATRIX A8): the original computed the `deej.unmapped` set when it
//! scanned sessions and reused it until the next scan. Here it's computed from the current sessions and mapping
//! every time, so it's never stale. The rule for what counts as unmapped is unchanged.

use std::collections::{BTreeMap, HashSet};

use crate::audio::{AudioSnapshot, EndpointRef, Flow, SYSTEM_SESSION_KEY, SessionInfo, VolumeTarget};
use crate::config::Config;

pub const MASTER: &str = "master";
pub const MIC: &str = "mic";
pub const SYSTEM: &str = SYSTEM_SESSION_KEY;
pub const CURRENT: &str = "deej.current";
pub const UNMAPPED: &str = "deej.unmapped";
const SPECIAL_PREFIX: &str = "deej.";

/// The original's device-name pattern `^.+ \(.+\)$` (e.g. "speakers (realtek audio)"). Sessions whose key looks
/// like this always counted as mapped.
fn looks_like_device_name(key: &str) -> bool {
    let Some(inner) = key.strip_suffix(')') else { return false };
    inner.match_indices(" (").any(|(at, _)| at >= 1 && at + 2 < inner.len())
}

/// Lowercased non-special targets of every slider: the names `deej.unmapped` must leave alone.
pub fn mapped_names(mapping: &BTreeMap<usize, Vec<String>>) -> HashSet<String> {
    mapping.values().flatten().map(|t| t.to_lowercase()).filter(|t| !t.starts_with(SPECIAL_PREFIX)).collect()
}

/// Whether `deej.unmapped` covers this session (before `unmapped_exclude`).
pub fn is_unmapped(session: &SessionInfo, mapped: &HashSet<String>) -> bool {
    let key = session.key.as_str();
    !matches!(key, MASTER | SYSTEM | MIC) && !looks_like_device_name(key) && !mapped.contains(key)
}

/// Resolves slider targets against the current audio state.
pub struct Resolver<'a> {
    snapshot: &'a AudioSnapshot,
    mapped: HashSet<String>,
    unmapped_exclude: &'a [String],
}

impl<'a> Resolver<'a> {
    pub fn new(config: &'a Config, snapshot: &'a AudioSnapshot) -> Resolver<'a> {
        Resolver { snapshot, mapped: mapped_names(&config.slider_mapping), unmapped_exclude: &config.unmapped_exclude }
    }

    /// Everything the given targets control, without duplicates. `foreground` is only called if a target is
    /// `deej.current`, and returns the foreground app's process names.
    pub fn resolve(&self, targets: &[String], foreground: &mut dyn FnMut() -> Vec<String>) -> Vec<VolumeTarget> {
        let mut result: Vec<VolumeTarget> = Vec::new();
        let mut push = |target: VolumeTarget| {
            if !result.contains(&target) {
                result.push(target);
            }
        };

        for target in targets {
            let target = target.to_lowercase();

            match target.as_str() {
                MASTER => push(VolumeTarget::Endpoint(EndpointRef::Default(Flow::Render))),
                MIC => push(VolumeTarget::Endpoint(EndpointRef::Default(Flow::Capture))),

                CURRENT => {
                    let names: HashSet<String> = foreground().into_iter().map(|n| n.to_lowercase()).collect();
                    for session in self.snapshot.sessions.iter().filter(|s| names.contains(&s.key)) {
                        push(VolumeTarget::Session(session.id));
                    }
                }

                UNMAPPED => {
                    for session in self.snapshot.sessions.iter() {
                        if is_unmapped(session, &self.mapped) && !self.unmapped_exclude.contains(&session.key) {
                            push(VolumeTarget::Session(session.id));
                        }
                    }
                }

                // unknown special targets do nothing, as in the original
                other if other.starts_with(SPECIAL_PREFIX) => {}

                name => {
                    for session in self.snapshot.sessions.iter().filter(|s| s.key == name) {
                        push(VolumeTarget::Session(session.id));
                    }
                    for endpoint in self.snapshot.endpoints.iter().filter(|e| e.name.to_lowercase() == name) {
                        push(VolumeTarget::Endpoint(EndpointRef::Id(endpoint.id.clone())));
                    }
                }
            }
        }

        result
    }

    /// The volume changes for one slider at `position`, after its sensitivity curve.
    pub fn slider_volumes(
        &self,
        config: &Config,
        slider: usize,
        position: f32,
        foreground: &mut dyn FnMut() -> Vec<String>,
    ) -> Vec<(VolumeTarget, f32)> {
        let Some(targets) = config.slider_mapping.get(&slider) else { return Vec::new() };
        let volume = config.slider_sensitivity.for_slider(slider).apply(position);
        self.resolve(targets, foreground).into_iter().map(|t| (t, volume)).collect()
    }
}

/// Of `targets`, the ones that didn't exist in `before` (new sessions or devices), or that now point at a different
/// device (`master`/`mic` after the default device changed). Used to bring newly started apps to their slider's
/// volume straight away (COMPATIBILITY_MATRIX A14/A15) without touching apps whose volume the user may have changed.
pub fn new_targets(before: &AudioSnapshot, after: &AudioSnapshot, targets: Vec<VolumeTarget>) -> Vec<VolumeTarget> {
    targets
        .into_iter()
        .filter(|target| match target {
            VolumeTarget::Session(id) => before.session(*id).is_none(),
            VolumeTarget::Endpoint(EndpointRef::Id(id)) => before.endpoint(id).is_none(),
            VolumeTarget::Endpoint(EndpointRef::Default(flow)) => {
                before.default_endpoint(*flow).map(|e| &e.id) != after.default_endpoint(*flow).map(|e| &e.id)
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::sample_snapshot;
    use crate::config;

    fn config(yaml: &str) -> Config {
        config::parse(yaml, None).unwrap().config
    }

    fn no_foreground() -> Vec<String> {
        panic!("foreground app looked up without a deej.current target")
    }

    fn sessions(targets: &[VolumeTarget]) -> Vec<u64> {
        targets
            .iter()
            .filter_map(|t| match t {
                VolumeTarget::Session(id) => Some(*id),
                _ => None,
            })
            .collect()
    }

    fn resolve(yaml: &str, slider: usize) -> Vec<VolumeTarget> {
        let config = config(yaml);
        let snapshot = sample_snapshot();
        let resolver = Resolver::new(&config, &snapshot);
        resolver.resolve(&config.slider_mapping[&slider], &mut no_foreground)
    }

    #[test]
    fn master_mic_and_system() {
        assert_eq!(
            resolve("slider_mapping:\n  0: [Master, MIC]\n", 0),
            vec![
                VolumeTarget::Endpoint(EndpointRef::Default(Flow::Render)),
                VolumeTarget::Endpoint(EndpointRef::Default(Flow::Capture))
            ]
        );
        assert_eq!(sessions(&resolve("slider_mapping:\n  0: system\n", 0)), vec![1]);
    }

    #[test]
    fn exe_names_match_every_session_case_insensitively_and_exactly() {
        assert_eq!(sessions(&resolve("slider_mapping:\n  0: CHROME.exe\n", 0)), vec![2, 3]);
        assert!(resolve("slider_mapping:\n  0: chrome\n", 0).is_empty());
        assert_eq!(
            sessions(&resolve("slider_mapping:\n  0: [chrome.exe, discord.exe, chrome.exe]\n", 0)),
            vec![2, 3, 4]
        );
    }

    #[test]
    fn device_names_target_that_device() {
        assert_eq!(
            resolve("slider_mapping:\n  0: \"headset earphone (corsair virtuoso se wireless gaming headset)\"\n", 0),
            vec![VolumeTarget::Endpoint(EndpointRef::Id("headset".into()))]
        );
    }

    #[test]
    fn unmapped_covers_every_app_no_slider_names() {
        let yaml = "slider_mapping:\n  0: master\n  1: chrome.exe\n  2: deej.unmapped\n";
        // discord and spotify; not chrome (mapped), not system
        assert_eq!(sessions(&resolve(yaml, 2)), vec![4, 5]);

        let excluded = format!("{yaml}unmapped_exclude: [Spotify.exe]\n");
        assert_eq!(sessions(&resolve(&excluded, 2)), vec![4]);
    }

    #[test]
    fn current_uses_the_foreground_processes() {
        let config = config("slider_mapping:\n  0: deej.current\n");
        let snapshot = sample_snapshot();
        let resolver = Resolver::new(&config, &snapshot);

        let targets = resolver
            .resolve(&config.slider_mapping[&0], &mut || vec!["ApplicationFrameHost.exe".into(), "Spotify.exe".into()]);
        assert_eq!(sessions(&targets), vec![5]);
    }

    #[test]
    fn unknown_special_targets_do_nothing() {
        assert!(resolve("slider_mapping:\n  0: deej.something\n", 0).is_empty());
    }

    #[test]
    fn device_name_pattern() {
        assert!(looks_like_device_name("speakers (realtek audio)"));
        assert!(looks_like_device_name("a (b)"));
        assert!(!looks_like_device_name("chrome.exe"));
        assert!(!looks_like_device_name(" (b)"));
        assert!(!looks_like_device_name("a ()"));
    }

    #[test]
    fn slider_volumes_apply_the_sensitivity_curve() {
        let config = config(
            "slider_mapping:\n  0: master\n  1: spotify.exe\nslider_sensitivity:\n  default:\n    curve: 2\n  1:\n    max_volume: 50\n",
        );
        let snapshot = sample_snapshot();
        let resolver = Resolver::new(&config, &snapshot);

        assert_eq!(resolver.slider_volumes(&config, 0, 0.5, &mut no_foreground)[0].1, 0.25);
        assert_eq!(
            resolver.slider_volumes(&config, 1, 0.5, &mut no_foreground),
            vec![(VolumeTarget::Session(5), 0.125)]
        );
        assert!(resolver.slider_volumes(&config, 9, 0.5, &mut no_foreground).is_empty());
    }

    #[test]
    fn new_targets_are_new_sessions_and_moved_defaults() {
        let before = sample_snapshot();
        let mut after = sample_snapshot();
        after.sessions.retain(|s| s.id != 5);
        let mut spotify = before.session(5).unwrap().clone();
        spotify.id = 6;
        after.sessions.push(spotify);

        let master = VolumeTarget::Endpoint(EndpointRef::Default(Flow::Render));
        let targets = vec![VolumeTarget::Session(2), VolumeTarget::Session(6), master.clone()];
        assert_eq!(new_targets(&before, &after, targets.clone()), vec![VolumeTarget::Session(6)]);

        after.endpoints[0].is_default = false;
        after.endpoints[1].is_default = true;
        assert_eq!(new_targets(&before, &after, targets), vec![VolumeTarget::Session(6), master]);
    }
}
